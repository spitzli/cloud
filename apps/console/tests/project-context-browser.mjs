// Owned customer HTTP fixtures; no real provider calls, credentials or writes.
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { fromRepo } from './repo.mjs';
const output=fileURLToPath(fromRepo('.runtime-test/customer-project-context-20261107'));
await mkdir(output,{recursive:true});
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
import neon from '@plugins/neon/manifest.json' with { type: 'json' };
import postgres from '@plugins/postgres/manifest.json' with { type: 'json' };
const browser = await chromium.launch({headless:true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? {executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE}: {})});
async function open({foreign=false, failed=false, appStatus="ready", locale="en"}={}) {
  const context=await browser.newContext({viewport:{width:1440,height:1000}}), page=await context.newPage(), calls=[], errors=[];
  page.setDefaultTimeout(12000);page.on('pageerror',e=>errors.push(e.message));
  await page.route(url=>url.pathname.startsWith('/api/') || ['/v1/auth/session','/console-config.json'].includes(url.pathname),route=>{
    const u=new URL(route.request().url()), json=data=>route.fulfill({json:data});
    if(u.pathname==='/console-config.json')return json({apiOrigin:previewOrigin,authOrigin:previewOrigin,enabledFeatures:['hosting'],localePreference:locale});
    if(u.pathname==='/v1/auth/session')return json({data:{user:{id:'2',name:'QA customer',role:'user',email:'qa@example.test',emailVerified:true},session:{id:'project-context-test',userId:'2',expiresAt:'2027-01-01T00:00:00Z'}}});
    if(u.pathname==='/api/console/tenants')return json([{id:'101',name:'Owned workspace',role:'owner',status:'active'}]);
    if(u.pathname==='/api/console/tenant/101/projects')return json([{id:'201',name:'Lunares bot',status:'active'},{id:'202',name:'Admin panel',status:'active'}]);
    if(/^\/api\/console\/project\/(201|202)$/.test(u.pathname)) {const id=u.pathname.split('/').at(-1);return json({id,name:id==='201'?'Lunares bot':'Admin panel',customer:{id:'101',name:'Owned workspace'},role:'owner',url:id==='202'?'https://admin.example.test':null,repositoryURL:'https://github.com/spitzli/lunares'});}
    if(u.pathname==='/api/console/plugins')return json([neon,postgres].map((manifest,index)=>({manifest,state:'enabled',runtime:'ready',features:[],resources:[{id:`database-${index}`,name:index?'Analytics DB':'Lunares DB',offering:manifest.offerings[0].id,createdVia:'operator'}]})));
    if(u.pathname==='/api/console/hosting') {
      const cmd=route.request().postDataJSON();calls.push(cmd);
      if(failed)return route.fulfill({status:503});
      if(cmd.action==='projects.get')return json({projectID:cmd.projectID,name:cmd.projectID==='201'?'Lunares bot':'Admin panel',customerID:'101',customerName:'Owned workspace',operator:false,configured:true,provider:cmd.projectID==='201'?'k3s':'vercel'});
      if(cmd.action==='apps.list')return json({page:1,limit:20,totalDocs:1,totalPages:1,subscriptionRevision:1,operator:false,canManage:true,ownImages:true,docs:[{id:'301',projectID:'201',name:'Lunares service',status:appStatus,revision:39,observedRevision:appStatus==='failed'?38:39,operationID:null,logs:null,lastError:null,lifecycle:'deployed',spec:{template:'custom',image:`registry.example/lunares@sha256:${'a'.repeat(64)}`,args:[],port:8080,healthPath:'/',cpuMillicores:100,memoryBytes:67108864,ephemeralBytes:67108864,replicas:1,volumeBytes:0}}]});
      if(cmd.action==='git.sources.list')return json({docs:[{id:'401',projectID:'201',targetID:'301',connectionID:'501',repositoryID:'601',repositoryName:'spitzli/lunares',branch:'main',rootDirectory:'.',recipe:'dockerfile',revision:1,enabled:true,autoPublish:false,executionBackend:'kubernetes',buildProvider:'isolated'}]});
      return route.fulfill({status:403});
    }
    return route.fulfill({status:404});
  });
  await page.goto(previewOrigin+`/workspaces/${foreign?'102':'101'}/projects/201`);
  return {page,context,calls,errors};
}
try {
  let s=await open();
  await s.page.getByRole('link',{name:'Lunares service',exact:true}).waitFor();
  await s.page.getByText('spitzli/lunares',{exact:true}).waitFor();
  await s.page.getByRole('link',{name:'Git builds and releases',exact:true}).waitFor();
  await s.page.screenshot({path:`${output}/project-fixture.png`,fullPage:true});
  const sidebar=s.page.getByRole('navigation',{name:'Primary navigation'});
  assert.equal(await sidebar.getByRole('link',{name:/^Neon/}).count(),0,'Neon is not a top-level customer destination');
  await sidebar.getByRole('link',{name:/^Databases/}).click();
  await s.page.waitForURL('**/workspaces/101/databases');
  await s.page.getByRole('heading',{name:'Databases',exact:true}).waitFor();
  await s.page.getByRole('link',{name:'Neon',exact:true}).waitFor();
  await s.page.getByRole('link',{name:'Lunares DB',exact:true}).first().waitFor();
  await s.page.screenshot({path:`${output}/databases-fixture.png`,fullPage:true});
  await s.page.setViewportSize({width:375,height:812});
  assert.equal(await s.page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
  await s.page.goto(previewOrigin+'/workspaces/101/projects/202');
  await s.page.getByRole('link',{name:'https://admin.example.test/',exact:true}).waitFor();
  await s.page.getByText('Vercel',{exact:true}).first().waitFor();
  assert.equal(s.calls.some(c=>c.projectID==='202' && ['apps.list','git.sources.list'].includes(c.action)),false,'Vercel must not invent native apps or build sources');
  assert.deepEqual(s.errors,[]);await s.context.close();
  s=await open({foreign:true});await s.page.getByText('This project is not available in this workspace.',{exact:true}).waitFor();assert.equal(s.calls.length,0);await s.context.close();
  s=await open({failed:true});await s.page.getByRole('alert').first().waitFor();await s.page.getByText('Lunares DB',{exact:true}).waitFor();assert.equal(await s.page.getByText('No applications',{exact:true}).count(),0);await s.context.close();
  s=await open({appStatus:'failed'});await s.page.getByRole('link',{name:'Lunares service',exact:true}).waitFor();await s.page.getByText('failed',{exact:true}).waitFor();assert.equal(await s.page.getByText('reconciling',{exact:true}).count(),0);await s.context.close();
  s=await open({locale:'de'});await s.page.goto(previewOrigin+'/workspaces/101/databases');await s.page.getByRole('heading',{name:'Datenbanken',exact:true}).waitFor();await s.page.getByRole('link',{name:'Neon',exact:true}).waitFor();await s.page.setViewportSize({width:375,height:812});assert.equal(await s.page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);await s.page.screenshot({path:`${output}/databases-de-mobile-fixture.png`,fullPage:true});assert.deepEqual(s.errors,[]);await s.context.close();
  console.log('PASS customer native app/Git context, Vercel metadata, database navigation, wrong-workspace gate and isolated failure states. Owned fixtures only.');
} finally {await browser.close();await server.close();}
