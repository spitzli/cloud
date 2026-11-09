// Owned HTTP fixtures: deliberate navigation intent may warm code/summary, never private resource actions.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
const browser=await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE?{executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE}:{})});
try{
  const context=await browser.newContext({viewport:{width:1280,height:900}});const page=await context.newPage();page.setDefaultTimeout(12000);const errors=[],calls=[],modules=[];
  page.on('pageerror',e=>errors.push(e.message));page.on('request',r=>{const p=new URL(r.url()).pathname;if(p.includes('Connections'))modules.push(p);});
  const path=p=>u=>u.pathname===p;
  await page.route(path('/console-config.json'),r=>r.fulfill({json:{apiOrigin:previewOrigin,authOrigin:previewOrigin,enabledFeatures:[]}}));
  await page.route(path('/v1/auth/session'),r=>r.fulfill({json:{data:{user:{id:'2',name:'Owner',email:'qa@example.test',role:'user',emailVerified:true},session:{id:'intent-session',userId:'2',expiresAt:'2027-01-01T00:00:00Z'}}}}));
  await page.route(u=>u.pathname.startsWith('/api/console/'),r=>{
    const u=new URL(r.request().url());calls.push({path:u.pathname,method:r.request().method()});const json=x=>r.fulfill({json:x});
    if(u.pathname==='/api/console/tenants')return json([{id:'101',name:'Alpha',role:'owner'}]);
    if(u.pathname==='/api/console/tenant/101/projects')return json([{id:'201',name:'Alpha project',status:'active'},{id:'202',name:'Beta project',status:'active'}]);
    if(u.pathname==='/api/console/project/201')return json({id:'201',name:'Alpha project',customer:{id:'101',name:'Alpha'},role:'owner'});
    if(u.pathname==='/api/console/project/202')return json({id:'202',name:'Beta project',customer:{id:'101',name:'Alpha'},role:'owner'});
    return json([]);
  });
  await page.goto(previewOrigin+'/workspaces/101');const project=page.getByRole('link',{name:'Alpha project',exact:true});await project.waitFor();
  assert.equal(calls.filter(x=>x.path==='/api/console/project/201').length,0,'project details do not load merely by entering viewport');
  await page.getByRole('link',{name:'Beta project',exact:true}).hover();await page.mouse.move(0,0);
  const summary=page.waitForResponse(r=>new URL(r.url()).pathname==='/api/console/project/201');await project.hover();await summary;
  assert.match(page.url(),/\/workspaces\/101$/,'intent does not navigate');assert.equal(calls.filter(x=>x.path==='/api/console/project/202').length,0,'unintended project is not warmed');assert.equal(calls.filter(x=>x.method!=='GET').length,0,'hover never dispatches actions');
  await project.focus();await project.click();await page.waitForURL('**/workspaces/101/projects/201');await page.getByRole('heading',{name:'Alpha project',exact:true}).waitFor();assert.equal(calls.filter(x=>x.path==='/api/console/project/201').length,1,'navigation consumes the same fresh summary');
  const connections=page.getByRole('link',{name:'Connections',exact:true}).first();await connections.waitFor();assert.equal(modules.length,0,'lazy code does not load simply by displaying a link');
  const code=page.waitForRequest(r=>new URL(r.url()).pathname.includes('Connections'));await connections.hover();await code;assert.match(page.url(),/\/projects\/201$/,'code preload does not navigate');assert.equal(calls.filter(x=>/connections|invoke|backup|logs/.test(x.path)).length,0,'code intent performs no connection, resource or action read');assert.deepEqual(errors,[]);
  await context.close();console.log('Intent prefetch PASS: hover-only scoped summary, focus/click dedup, lazy route code before navigation, no unrelated actions. Owned fixtures only.');
}finally{await browser.close();await server.close();}
