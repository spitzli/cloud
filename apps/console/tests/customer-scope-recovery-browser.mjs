// Local UI regression: server commits customer mode, then responses are lost/stalled.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin as origin } from './preview-server.ts';
const browser=await chromium.launch({headless:true});
try {
 for(const recovery of ['success','stall']) {
  const page=await browser.newPage();let acting=false;let starts=0;
  await page.route(`${origin}/v1/auth/session`,route=>route.fulfill({json:{data:{user:{id:'1',name:'Acceptance operator',email:'operator@example.test',role:acting?'user':'operator',emailVerified:true,preview:acting,acting,customerScope:acting?'101':null},session:{id:'test-session',userId:'1',expiresAt:'2030-01-01T00:00:00Z'}}}}));
  await page.route(`${origin}/api/console/tenant-preview/current`,route=>recovery==='stall'?undefined:route.fulfill({json:{customerID:'101',customerName:'Acceptance customer',status:'active',mode:'owner',readOnly:false,expiresAt:'2030-01-01T00:00:00Z'}}));
  await page.route(`${origin}/api/console/tenant-preview/start`,async route=>{assert.equal(route.request().postDataJSON().customerID,'101');starts++;acting=true;await route.abort('failed');});
  await page.goto(`${origin}/customers/101`);
  await page.getByRole('button',{name:'Act as customer owner',exact:true}).click();
  if(recovery==='success') await page.waitForURL(`${origin}/workspaces/101`,{timeout:10000});
  else await page.waitForFunction(()=>performance.getEntriesByType('navigation')[0]?.type==='reload',undefined,{timeout:12000});
  await page.getByRole('complementary',{name:'Customer session'}).getByText('Acting as customer owner',{exact:true}).waitFor();
  assert.equal(starts,1,'recover or bootstrap, never blindly repeat the start mutation');
  assert.equal(await page.getByRole('button',{name:'Act as customer owner',exact:true}).count(),0,'stale operator controls must disappear');
  console.log(`Committed customer-mode ${recovery} recovery: one start, fresh authority, no stale operator UI. PASS`);
  await page.close();
 }
}finally{await browser.close();await server.close();}
