// Real first-party operator cookie from the independent account-browser MFA/PKCE fixture.
import assert from 'node:assert/strict';
import { readFileSync, mkdirSync } from 'node:fs';
import { fromRepo } from './repo.mjs';
import { chromium } from 'playwright';
import { fileURLToPath } from 'node:url';
import { accountCookies } from './account-fixture.mjs';
const origin='http://127.0.0.1:3121';
const browser=await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE?{executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE}:{})});
const context=await browser.newContext({viewport:{width:1280,height:900}});
const page=await context.newPage();
const errors=[];page.on('pageerror',error=>errors.push(error.message));
await context.addCookies(JSON.parse(readFileSync(accountCookies,'utf8')));
const request=async(path,body)=>{
  const response=await page.request.post(origin+path,{data:body,headers:{Origin:origin}});
  return {status:response.status(),data:await response.json()};
};
async function removeCustomer(customer) {
  const response=await page.request.get(origin+`/api/console/tenant/${customer.id}/projects`);
  if(response.ok())for(const project of await response.json())await page.request.delete(origin+`/api/console/registry/projects/${project.id}`,{headers:{Origin:origin}});
  await page.request.delete(origin+`/api/console/registry/customers/${customer.id}`,{headers:{Origin:origin}});
}
const customers=[];
try {
  await page.goto(origin+'/customers');
  await page.getByRole('heading',{name:'Customers',exact:true}).waitFor();
  const before=(await (await page.request.get(origin+'/api/console/session')).json()).data;
  assert.equal(before.user.role,'operator');
  assert.equal(before.user.preview,false);
  for(const label of ['Selected','Foreign']){
    const created=await request('/api/console/registry/customers',{name:`Acting browser ${label} ${Date.now()}`,customerType:'company'});
    assert.equal(created.status,200);
    customers.push(created.data);
  }
  const [selected,foreign]=customers;
  await page.goto(origin+`/customers/${selected.id}`);
  await page.getByRole('button',{name:'Act as customer owner',exact:true}).click();
  await page.waitForURL(origin+`/workspaces/${selected.id}`);
  const banner=page.getByRole('complementary',{name:'Customer session'});
  await banner.getByText('Acting as customer owner',{exact:true}).waitFor();
  await banner.getByText(new RegExp(selected.name)).waitFor();
  const evidence=fileURLToPath(fromRepo('.runtime-test/acting'));mkdirSync(evidence,{recursive:true,mode:0o700});
  for(const width of [375,1280]) { await page.setViewportSize({width,height:900});await page.getByRole('heading',{name:selected.name,exact:true}).waitFor();await page.screenshot({path:`${evidence}/owner-${width}.png`,fullPage:true}); }
  const acting=(await (await page.request.get(origin+'/api/console/session')).json()).data;
  assert.equal(acting.user.id,before.user.id);
  assert.equal(acting.session.id,before.session.id);
  assert.equal(acting.user.role,'user');assert.equal(acting.user.preview,true);assert.equal(acting.user.acting,true);
  assert.equal(await page.getByRole('link',{name:'Customers',exact:true}).count(),0);
  assert.equal(await page.getByRole('link',{name:'API tokens',exact:true}).count(),0);
  await page.getByRole('button',{name:'Add a project',exact:true}).click();
  const name=`Acted project ${Date.now()}`;
  await page.getByRole('textbox',{name:'Project name',exact:true}).fill(name);
  await page.getByRole('button',{name:'Create project',exact:true}).click();
  await page.getByRole('heading',{name,exact:true}).waitFor();
  assert.equal((await request(`/api/console/tenant/${foreign.id}/projects`,{name:'Denied foreign project'})).status,404);
  assert.equal((await page.request.get(origin+'/api/console/registry/customers')).status(),403);
  await banner.getByRole('button',{name:'Exit customer mode',exact:true}).click();
  await page.waitForURL(origin+'/customers');
  await page.getByRole('heading',{name:'Customers',exact:true}).waitFor();
  const after=(await (await page.request.get(origin+'/api/console/session')).json()).data;
  assert.equal(after.user.role,'operator');assert.equal(after.user.preview,false);
  assert.equal(after.session.id,before.session.id);
  await page.goto(origin+`/customers/${selected.id}`);
  await page.getByRole('button',{name:'Preview read-only',exact:true}).click();
  await page.waitForURL(origin+`/workspaces/${selected.id}`);
  await banner.getByText('Read-only customer preview',{exact:true}).waitFor();
  assert.equal(await page.getByRole('button',{name:'New project',exact:true}).count(),0);
  assert.equal(await page.getByRole('button',{name:'Add a project',exact:true}).count(),0);
  assert.equal((await request(`/api/console/tenant/${selected.id}/projects`,{name:'Denied readonly project'})).status,403);
  await banner.getByRole('button',{name:'Exit customer mode',exact:true}).click();
  await page.waitForURL(origin+'/customers');
  assert.deepEqual(errors,[]);
  console.log('Real acting browser passed: native MFA/PKCE cookie, explicit owner/read-only transitions, original identity, project create, foreign/global denial and operator restoration.');
} finally {
  // This test owns only the exact two customers it created on the disposable fixture.
  await request('/api/console/tenant-preview/exit',{}).catch(()=>{});
  for(const customer of customers)await removeCustomer(customer);
  await context.close();await browser.close();
}
