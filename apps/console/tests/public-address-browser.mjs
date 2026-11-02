// Owned browser fixture only; real RLS and ingress are separately qualified.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {server,previewOrigin} from './preview-server.ts';
const browser=await chromium.launch({headless:true});
try {
 for(const [enabled,privateService] of [[true,false],[true,true],[false,false]]) {
  const context=await browser.newContext({viewport:{width:375,height:812}});
  const page=await context.newPage();page.setDefaultTimeout(15000);const errors=[];page.on('pageerror',e=>errors.push(e.message));
  const json=(value)=>route=>route.fulfill({json:value});const path=p=>url=>url.pathname===p;let submitted;
  await page.route(path('/console-config.json'),json({apiOrigin:previewOrigin,authOrigin:previewOrigin,enabledFeatures:['hosting']}));
  await page.route(path('/v1/auth/session'),json({data:{user:{id:'2',name:'Fixture Owner',email:'owner@example.invalid',role:'operator',emailVerified:true},session:{id:'s',userId:'2',expiresAt:'2026-12-01T00:00:00Z'}}}));
  await page.route(path('/api/console/tenants'),json([{id:'101',name:'Owned fixture',status:'active',role:'owner'}]));
  await page.route(path('/api/console/tenant/101/projects'),json([{id:'201',name:'Public project',status:'active'}]));
  await page.route(path('/api/console/plugins'),json([]));
  await page.route(path('/api/console/tenant/101/domains'),json([]));
  await page.route(path('/api/console/hosting'),route=>{
   const cmd=route.request().postDataJSON();
   const reply=value=>route.fulfill({json:value});
   if(cmd.action==='projects.get')return reply({projectID:'201',name:'Public project',customerID:'101',customerName:'Owned fixture',operator:true,configured:true,provider:'k3s',publicSitePolicy:{domain:'spitzli.site',enabled}});
   if(cmd.action==='apps.list')return reply({page:1,limit:20,totalDocs:0,totalPages:0,docs:[],subscriptionRevision:1,operator:true,canManage:true,ownImages:true});
   if(cmd.action==='apps.create'){submitted=cmd;return reply({id:'301',appID:'401',status:'queued',revision:1})}
   if(cmd.action==='operations.get')return reply({id:'301',action:'apps.create',status:'queued',customerID:'101',projectID:'201',createdAt:'2026-10-31T12:00:00Z'});
   if(cmd.action==='addresses.list')return reply({docs:[{id:'501',hostname:'public-project.spitzli.site',url:'https://public-project.spitzli.site',state:'ready',revision:1,projectName:'Public project',appName:'My app'}],totalDocs:1});
   return route.fulfill({status:404,json:{error:{message:'Unexpected owned fixture command'}}});
  });
  await page.goto(previewOrigin+'/hosting/projects/201');
  await page.getByRole('button',{name:'New application',exact:true}).click();
  const form=page.getByRole('region',{name:'New application',exact:true});
  if(enabled)assert.equal(await form.getByRole('checkbox',{name:/Public website or app/}).isChecked(),true);
  else assert.equal(await form.getByRole('checkbox',{name:/Public website or app/}).count(),0);
  if(enabled&&privateService)await form.getByRole('checkbox',{name:/Public website or app/}).uncheck();
  await form.getByRole('textbox',{name:'Application name',exact:true}).fill('My app');
  await form.getByRole('textbox',{name:'Image digest',exact:true}).fill('registry.example/app@sha256:'+'a'.repeat(64));
  await form.getByRole('button',{name:'Request application',exact:true}).click();
  await page.getByRole('link',{name:'Open new application',exact:true}).waitFor();
  assert.ok(submitted);assert.equal(Boolean(submitted.publicAddress),enabled&&!privateService);
  await page.goto(previewOrigin+'/workspaces/101/domains');
  const link=page.getByRole('link',{name:'public-project.spitzli.site',exact:true});await link.waitFor();assert.equal(await link.getAttribute('href'),'https://public-project.spitzli.site');
  for(const width of [320,375,414,768,1280]){await page.setViewportSize({width,height:812});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false)}
  assert.deepEqual(errors,[]);await context.close();
 }
 console.log('Public address UI: enabled automatic request, private opt-out, disabled policy, assigned URL and responsive widths passed. Synthetic HTTP fixture only.');
} finally {await browser.close();await server.close()}
