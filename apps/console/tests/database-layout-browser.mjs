// Controlled local HTTP only; genuine Console shell and retained Tabularis UI, no customer data.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin as origin } from './preview-server.ts';
const browser = await chromium.launch({ headless: true });
try {
 const page = await browser.newPage({viewport:{width:1440,height:900}});
 await page.route(`${origin}/console-config.json`, route=>route.fulfill({json:{apiOrigin:origin,authOrigin:origin,enabledFeatures:['databases','hosting']}}));
 await page.route(`${origin}/api/console/databases`, route=>route.fulfill({json:{data:{code:'a'.repeat(43),gatewayOrigin:origin}}}));
 await page.route(`${origin}/api/console/tenants`,route=>route.fulfill({json:[{id:'101',name:'Acceptance customer',organizationId:'102',role:'owner'}]}));
 await page.route(`${origin}/session`, route=>route.fulfill({json:{scopeID:'owned',connectionID:'connection',baseUrl:`${origin}/s/owned`}}));
 await page.route(`${origin}/s/owned/api/v1/session`, route=>route.fulfill({json:{apiVersion:'v1',serverVersion:'fixture',serverBuild:{target:'owned',profile:'debug',commit:null},authenticated:true,csrfToken:'owned-csrf',access:{remote:true,authorizationLevel:'database',highRiskCapabilities:false},capabilities:{rpc:true,events:false,uploads:false,downloads:false,pluginAssets:false,mcpHostConfiguration:false,nativeUpdater:false},queryResponsePolicy:{maxRowsPerPage:100,maxResponseBytes:100000,streaming:false}}}));
 await page.route(`${origin}/s/owned/api/v1/rpc/**`, route=>route.fulfill({json:{ok:false,error:{code:'FIXTURE',message:'Controlled UI fixture',details:null,requestId:'owned'}}}));
 await page.route(`${origin}/s/owned/api/v1/logout`,route=>route.fulfill({json:{}}));
 await page.route(`${origin}/v1/auth/session`,route=>route.fulfill({json:{data:{user:{id:'1',name:'Acceptance operator',email:'operator@example.test',role:'user',emailVerified:true,preview:true,acting:true,customerScope:'101'},session:{id:'test-session',userId:'1',expiresAt:'2030-01-01T00:00:00Z'}}}}));
 await page.route(`${origin}/api/console/tenant-preview/current`,route=>route.fulfill({json:{customerID:'101',customerName:'Owned customer fixture',status:'active',mode:'owner',readOnly:false,expiresAt:'2030-01-01T00:00:00Z'}}));
 await page.goto(`${origin}/databases/123?customer=101`);
 await page.getByRole('heading',{name:'Database workspace',exact:true}).waitFor();
 assert.equal(await page.getByRole('navigation',{name:'Primary navigation',exact:true}).isVisible(),false,'viewer must use full width without console sidebar');
 await page.getByRole('complementary',{name:'Customer session'}).getByRole('button',{name:'Exit customer mode'}).waitFor();
 await page.locator('.tabularis-workspace').waitFor({timeout:60000});
 await page.getByRole('navigation',{name:'Breadcrumb'}).getByText('Acceptance customer',{exact:true}).waitFor({timeout:5000});
 for (const [width,height] of [[1440,900],[390,844]]) {
  await page.setViewportSize({width,height});
  for(const theme of ['light','dark']) {
   await page.evaluate(theme=>document.documentElement.dataset.theme=theme,theme);
   await page.waitForFunction(()=>{const w=document.querySelector('.tabularis-workspace');return w && getComputedStyle(w).getPropertyValue('--bg-base').trim()===getComputedStyle(document.documentElement).getPropertyValue('--panel').trim();});
   const geometry=await page.evaluate(()=>{const main=document.querySelector('#main').getBoundingClientRect();const w=document.querySelector('.tabularis-workspace');const r=w.getBoundingClientRect();return{mainWidth:main.width,bottom:r.bottom,height:r.height,body:document.documentElement.scrollWidth,accent:getComputedStyle(w).getPropertyValue('--accent-primary').trim(),hostAccent:getComputedStyle(document.documentElement).getPropertyValue('--accent').trim()};});
   assert.equal(geometry.mainWidth,width);assert(geometry.height>height*.55);assert(geometry.bottom<=height+1);assert(geometry.body<=width);assert.equal(geometry.accent,geometry.hostAccent);
  }
 }
 await page.setViewportSize({width:1440,height:900});
 await page.getByRole('link',{name:'Close',exact:true}).click();
 await page.getByRole('navigation',{name:'Primary navigation',exact:true}).waitFor({state:'visible'});
 assert.equal(new URL(page.url()).pathname,'/databases');
 assert.equal(JSON.parse(new URL(page.url()).searchParams.get('customer')),'101');
 await page.route(`${origin}/api/console/hosting`,route=>route.fulfill({json:{operator:false,page:1,limit:20,totalDocs:2,totalPages:1,docs:[{projectID:'201',name:'Bot fixture',provider:'k3s',mode:'selfservice',clusterID:'401',clusterName:null,revision:1,ownImages:true},{projectID:'202',name:'Panel fixture',provider:'vercel',mode:'managed',clusterID:null,clusterName:null,revision:1,ownImages:false}]}}));
 await page.goto(`${origin}/hosting?customer=101`);
 await page.getByRole('cell',{name:'Managed cluster',exact:true}).waitFor({timeout:5000});
 await page.getByRole('cell',{name:'Vercel',exact:true}).waitFor({timeout:5000});
 assert.equal(await page.getByRole('cell',{name:'No cluster assigned',exact:true}).count(),0);
 await page.unroute(`${origin}/api/console/hosting`);
 let vercelAppReads=0;
 await page.route(`${origin}/api/console/hosting`,route=>{
  const action=route.request().postDataJSON().action;
  if(action==='projects.get')return route.fulfill({json:{projectID:'202',name:'Panel fixture',customerID:'101',customerName:'Acceptance customer',operator:false,configured:true,provider:'vercel'}});
  if(action==='apps.list'){vercelAppReads++;return route.fulfill({json:{docs:[],page:1,limit:20,totalDocs:0,totalPages:1,subscriptionRevision:1,operator:false,canManage:false,ownImages:false}});}
  return route.fulfill({status:400,json:{error:'Unexpected controlled action'}});
 });
 await page.goto(`${origin}/hosting/projects/202`);
 await page.getByRole('heading',{name:'Hosted on Vercel',exact:true}).waitFor({timeout:5000});
 await page.getByRole('button',{name:'Refresh',exact:true}).click();
 await page.getByRole('heading',{name:'Hosted on Vercel',exact:true}).waitFor();
 assert.equal(vercelAppReads,0,'external Vercel hosting must not poll a native application directory');
 assert.equal(await page.getByText('No applications',{exact:true}).count(),0);

 console.log('Full-page viewer, mobile geometry, light/dark Spitzli tokens and return navigation PASS (controlled local UI only).');
} finally {await browser.close();await server.close();}
