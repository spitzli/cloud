import {chromium} from 'playwright';
import {readFile,writeFile} from 'node:fs/promises';
import assert from 'node:assert/strict';
import pg from 'pg';
import {parseEnv} from 'node:util';
const fixture=JSON.parse(await readFile('.runtime-test/database-browser.json','utf8'));
const browser=await chromium.launch({headless:true});
const context=await browser.newContext({viewport:{width:1440,height:1000},locale:'en-GB'});
await context.addCookies(JSON.parse(await readFile('.runtime-test/account-browser-cookies.json','utf8')));
const page=await context.newPage();const errors=[];const deniedOptional=new Set();page.on('response',response=>{if(response.status()>=400){const path=new URL(response.url()).pathname;const command=path.split('/').pop();if(response.status()===403&&['get_keybindings','get_installed_plugins'].includes(command))deniedOptional.add(command);else errors.push(`${path}: ${response.status()}`);}});page.on('pageerror',error=>errors.push(error.message));page.on('console',message=>{if(message.type()==='error'&&!message.text().includes('the server responded with a status of 403'))errors.push(message.text().replace(/data:[^' ]+/g,'data:[bundled font]').slice(0,600));});
try{
 await page.goto('http://127.0.0.1:3121/databases?customer='+fixture.customerID);
 await page.getByRole('heading',{name:'Databases',exact:true}).waitFor();
 const opening=page.waitForResponse(response=>response.url()===fixture.gatewayOrigin+'/session');
 await page.getByRole('link',{name:'Open database',exact:true}).click();
 const opened=await(await opening).json();
 await page.waitForFunction(()=>document.querySelector('.database-workbench'));
 await page.locator('.monaco-editor .view-lines').first().click();
 await page.keyboard.press('ControlOrMeta+A');
 await page.keyboard.type('SELECT id, customer, total FROM orders;');
 await page.getByRole('button',{name:'Run (Ctrl+Enter)',exact:true}).click();
 await page.getByText('Local fixture',{exact:true}).waitFor();
 await page.screenshot({path:'.runtime-test/database-workbench.png',fullPage:true});
 const text=await page.locator('#main').innerText();
 assert.ok(!text.includes('Something went wrong'),text.slice(0,500));
 assert.ok(!text.includes('Database access is unavailable'),text.slice(0,500));
 assert.equal(errors.length,0,errors.join('\n'));
 assert.ok(!(await page.content()).includes(fixture.proxySecret));
 const storage=await page.evaluate(()=>Object.entries(localStorage));assert.ok(storage.every(([key,value])=>!/(access.?token|refresh.?token|session.?token)/i.test(key)&&!value.includes('Bearer ')));
 await page.setViewportSize({width:390,height:844});await page.screenshot({path:'.runtime-test/database-workbench-mobile.png',fullPage:true});assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
 await page.setViewportSize({width:1440,height:1000});
 await writeFile('.runtime-test/database-browser-state.json',JSON.stringify(await context.storageState()),{mode:0o600});

 const env=parseEnv(await readFile('.runtime-test/identity-restricted.env','utf8'));const url=new URL(env.WEBDOCK_TEST_MIGRATION_DATABASE_URL);assert.equal(url.hostname,'127.0.0.1');assert.equal(url.pathname,'/webdock_rls_test');assert.ok(!url.search&&!url.hash);
 const db=new pg.Client({connectionString:url.toString()});await db.connect();let original,reference;
 try {
   const proof=await db.query('SELECT native_authority FROM webdock_auth.database_session WHERE id=$1 AND grant_id=$2',[opened.scopeID,fixture.grantID]);assert.equal(proof.rowCount,1);const authority=proof.rows[0].native_authority;assert.equal(authority.kind,'first-party-browser');
   reference=authority.browser.reference;original=(await db.query('SELECT expires_at FROM webdock_auth.identity_browser_session WHERE token_hash=$1 AND user_id=$2',[reference,fixture.userID])).rows[0];assert.ok(original);
   await db.query('UPDATE webdock_auth.identity_browser_session SET expires_at=statement_timestamp()-interval \'1 second\' WHERE token_hash=$1 AND user_id=$2',[reference,fixture.userID]);
   const revoked=await context.request.get(opened.baseUrl+'/api/v1/session',{headers:{Origin:'http://127.0.0.1:3121'}});assert.ok([401,403].includes(revoked.status()),'gateway must revalidate the saved first-party browser reference');
 } finally {if(original)await db.query('UPDATE webdock_auth.identity_browser_session SET expires_at=$3 WHERE token_hash=$1 AND user_id=$2',[reference,fixture.userID,original.expires_at]);await db.end();}
 console.log('Rust Console→Core→gateway→SQLite query passed; saved first-party authority revocation denied the live gateway session.');
}finally{await browser.close();}
