// Owned HTTP fixtures only: exercise actual Console components, never production.
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
import { fromRepo } from './repo.mjs';
import { fileURLToPath } from 'node:url';
const output=fileURLToPath(fromRepo('.runtime-test/application-autoscaling-20261108'));
await mkdir(output,{recursive:true});
const browser=await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE?{executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE}:{})});
let current;
async function open({available=true,volume=0}={}) {
  const context=await browser.newContext({viewport:{width:1280,height:900}}), page=await context.newPage();
  const errors=[],writes=[];page.on('pageerror',e=>errors.push(e.message));page.setDefaultTimeout(12000);
  const app={id:'301',projectID:'201',name:'Customer API',lifecycle:'deployed',revision:1,observedRevision:1,status:'ready',operationID:null,logs:null,lastError:null,environmentNames:[],subscriptionRevision:1,operator:false,canManage:true,canRequestLogs:true,automaticScalingAvailable:available,spec:{template:'custom',image:`registry.example/app@sha256:${'a'.repeat(64)}`,args:[],port:8080,healthPath:'/',cpuMillicores:100,memoryBytes:67108864,ephemeralBytes:67108864,replicas:1,volumeBytes:volume}};
  await page.route(u=>u.pathname.startsWith('/api/')||['/v1/auth/session','/console-config.json'].includes(u.pathname),route=>{
    const path=new URL(route.request().url()).pathname,json=data=>route.fulfill({json:data});
    if(path==='/console-config.json')return json({apiOrigin:previewOrigin,authOrigin:previewOrigin,enabledFeatures:['hosting']});
    if(path==='/v1/auth/session')return json({data:{user:{id:'2',name:'QA customer',email:'qa@example.test',emailVerified:true,role:'user'},session:{id:'test-session',userId:'2',expiresAt:'2027-01-01T00:00:00Z'}}});
    if(path==='/api/console/tenants')return json([{id:'101',name:'Customer',role:'owner',status:'active'}]);
    if(path==='/api/console/tenant/101/projects')return json([{id:'201',name:'API',status:'active'}]);
    if(path==='/api/console/project/201')return json({id:'201',name:'API',customer:{id:'101',name:'Customer'},role:'owner'});
    if(path==='/api/console/plugins')return json([]);
    if(path==='/api/console/hosting/reads/802')return json({id:'802',status:'succeeded',error:null,result:{observedAt:1800000000,autoscaling:{minReplicas:2,maxReplicas:4,currentReplicas:3,readyReplicas:3,desiredReplicas:3,metricsAvailable:false,reason:'MetricsUnavailable'}}});
    if(path==='/api/console/hosting') {
      const cmd=route.request().postDataJSON();
      if(cmd.action==='apps.get')return json(app);
      if(cmd.action==='projects.get')return json({projectID:'201',name:'API',customerID:'101',customerName:'Customer',operator:false,configured:true,provider:'k3s',automaticScalingAvailable:available});
      if(cmd.action==='operations.get')return json({id:'801',action:'apps.update',status:'succeeded',customerID:'101',projectID:'201',createdAt:null});
      if(cmd.action==='apps.inspect')return json({id:'802',appID:'301',status:'queued'});
      if(cmd.action==='apps.update') {writes.push(cmd);app.spec=cmd.spec;app.revision++;app.observedRevision=app.revision;return json({id:'801',appID:'301',status:'queued',revision:app.revision});}
    }
    return route.fulfill({status:404,json:{error:{message:'Fixture route unavailable'}}});
  });
  await page.goto(previewOrigin+'/hosting/apps/301/settings');
  await page.getByRole('heading',{name:'Application settings',exact:true}).waitFor();
  current={page,context,errors,writes,app}; return current;
}
try {
  let f=await open();
  await f.page.getByRole('combobox',{name:'Scaling mode'}).selectOption('automatic');
  await f.page.getByRole('spinbutton',{name:'Minimum instances',exact:true}).fill('2');
  await f.page.getByRole('spinbutton',{name:'Maximum instances',exact:true}).fill('4');
  await f.page.getByRole('button',{name:'Request update',exact:true}).click();
  await f.page.getByText('Request recorded',{exact:true}).waitFor();
  assert.deepEqual(f.writes[0].spec.autoscaling,{minReplicas:2,maxReplicas:4,targetCPUUtilizationPercent:70});
  await f.page.goto(previewOrigin+'/hosting/apps/301');
  await f.page.getByRole('button',{name:'Refresh live instances'}).click();
  await f.page.getByText('CPU metrics are temporarily unavailable. A new scaling target cannot be calculated yet.',{exact:true}).waitFor();
  assert.equal(await f.page.getByRole('button',{name:'Scale application',exact:true}).isDisabled(),true);
  await f.page.screenshot({path:output+'/scaling-desktop.png',fullPage:true});
  await f.page.setViewportSize({width:375,height:812});
  assert.equal(await f.page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
  await f.page.screenshot({path:output+'/scaling-mobile.png',fullPage:true});
  assert.deepEqual(f.errors,[]);await f.context.close();
  for(const options of [{available:false},{volume:67108864}]) {
    f=await open(options);
    assert.equal(await f.page.getByRole('option',{name:'Automatic scaling',exact:true}).evaluate(el=>el.disabled),true);
    assert.equal(await f.page.getByRole('combobox',{name:'Scaling mode'}).inputValue(),'fixed');
    assert.deepEqual(f.errors,[]);await f.context.close();
  }
  console.log('PASS autoscaling customer form, policy submission, runtime snapshot, missing metrics, mobile layout and capability/storage restrictions.');
} catch(error) { console.error(JSON.stringify({writes:current?.writes,errors:current?.errors,text:current?await current.page.locator('body').innerText():''}));if(current)await current.page.screenshot({path:output+'/browser-failure.png',fullPage:true});throw error; } finally {await browser.close();await server.close();}
