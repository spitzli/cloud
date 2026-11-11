// Account chrome and developer access use HTTP fixtures, never a live identity.
import assert from 'node:assert/strict';
import { mkdirSync } from 'node:fs';
import { chromium } from 'playwright';
import { server, previewOrigin as origin } from './preview-server.ts';
import { fromRepo } from './repo.mjs';
const browser=await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE?{executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE}:{})});
const errors=[];
try {
 const context=await browser.newContext({permissions:['clipboard-read','clipboard-write']});
 const page=await context.newPage();page.on('pageerror',error=>errors.push(error.message));
 let user={id:'1',name:'Ada Lovelace',email:'ada.with.a.long.address@example.test',role:'user',emailVerified:true,twoFactorEnabled:true};
 const json=value=>route=>route.fulfill({json:value});
 await page.route('**/console-config.json',json({apiOrigin:origin,authOrigin:origin,enabledFeatures:[],surface:'account',consoleOrigin:'https://console.example.test'}));
 await page.route('**/api/auth/get-session',route=>route.fulfill({json:user?{user,session:{id:'s1',userId:'1',expiresAt:'2099-01-01T00:00:00Z'}}:null}));
 await page.route('**/api/auth/list-sessions',json([{id:'s1',current:true,expiresAt:'2099-01-01T00:00:00Z',createdAt:'2026-11-11T00:00:00Z'}]));
 await page.route('**/api/auth/passkey/list-user-passkeys',json([]));
 await page.route('**/api/tenants',json([{id:'101',name:'Ada Studio',role:'owner',status:'active'},{id:'102',name:'Client workspace',role:'admin',status:'active'},{id:'103',name:'Shared project',role:'member',status:'active'}]));
 await page.route('**/api/invitations',json([]));
 mkdirSync(fromRepo('.runtime-test/accounts/screens'),{recursive:true});
 const cases=['light','dark'].flatMap(theme=>['/account','/workspaces','/developers'].flatMap(path=>[320,375,414,768,1280].map(width=>({theme,path,width}))));
 let previousTheme;
 for(const {theme,path,width} of cases){
  if(theme!==previousTheme){await page.addInitScript(value=>localStorage.setItem('webdock.theme.v1',value),theme);previousTheme=theme;}
    await page.setViewportSize({width,height:900});await page.goto(origin+path);
    await page.getByRole('link',{name:'Spitzli Account',exact:true}).waitFor().catch(async error=>{ console.log({url:page.url(),errors,body:await page.locator('body').innerText()});throw error; });
    await page.getByRole('navigation',{name:'Account',exact:true}).getByRole('link',{name:path==='/account'?'Security':path==='/developers'?'Developer access':'Workspaces',exact:true}).waitFor();
    assert.equal(await page.title(),'Spitzli Account');
    await page.evaluate(()=>document.fonts.ready);
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${path} ${theme} ${width}: no horizontal scroll`);
    const clipped=await page.locator('.account-navigation a, .account-brand').evaluateAll(nodes=>nodes.some(node=>{const r=node.getBoundingClientRect();return r.left<0||r.right>innerWidth||r.height<44;}));
    assert.equal(clipped,false,'account navigation fits and has touch targets');
    if(path==='/developers'){
     assert.equal(await page.getByLabel('Server URL',{exact:true}).inputValue(),'https://console.example.test/api/mcp');
     const links=await page.getByRole('link',{name:'Manage tokens',exact:true}).evaluateAll(nodes=>nodes.map(node=>node.getAttribute('href')));
     assert.deepEqual(links,['https://console.example.test/api/sso/login?returnTo=%2Fworkspaces%2F101%2Fapi-tokens','https://console.example.test/api/sso/login?returnTo=%2Fworkspaces%2F102%2Fapi-tokens']);
    }
    if(width===375||width===1280)await page.screenshot({path:fromRepo(`.runtime-test/accounts/screens/${path.slice(1)}-${theme}-${width}.png`).pathname,fullPage:true});
 }
 await page.getByRole('button',{name:'Copy server URL',exact:true}).click();
 await page.getByText('Server URL copied.',{exact:true}).waitFor();
 assert.equal(await page.evaluate(()=>navigator.clipboard.readText()),'https://console.example.test/api/mcp');
 // A failed workspace inventory is visible and retryable, with no invented empty list.
 await page.route('**/api/tenants',route=>route.fulfill({status:503,json:{error:{message:'unavailable'}}}));
 await page.reload();await page.getByRole('button',{name:'Try again',exact:true}).waitFor();
 user=null;await page.goto(origin+'/login');await page.getByRole('heading',{name:'Sign in to Spitzli Cloud',exact:true}).waitFor();
 assert.ok(await page.getByRole('link',{name:'Spitzli Account',exact:true}).isVisible());
 assert.equal(await page.getByRole('navigation',{name:'Account',exact:true}).count(),0,'signed-out flow stays focused');
 await page.getByRole('button',{name:'Forgot password?',exact:true}).click();
 await page.getByRole('heading',{name:'Reset your password',exact:true}).waitFor();
 assert.deepEqual(errors,[]);
 console.log('Account design: 30 route/theme/viewport checks, own identity, administrator-only token links, MCP copy, failed inventory and signed-out recovery. HTTP fixtures only.');
 await context.close();
}finally{await browser.close();await server.close();}
