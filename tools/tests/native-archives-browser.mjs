// Existing repository Playwright qualification workflow; real operator password/TOTP/PKCE cookies only.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { chmod } from 'node:fs/promises';
import { load } from './native-archives-fixture.mjs';
import { client, invoke } from './native-archives-acceptance.mjs';
const f=await load(process.argv[2]);let browser;
try{
 const c=await client(f);assert(c.state.resource&&f.report.signedNativeInstallation&&f.report.dataProof);
 browser=await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE?{executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE}:{})});
 const context=await browser.newContext({viewport:{width:1280,height:900}});
 const cookies=Object.entries(c.state.cookies).filter(([,value])=>value.slice(value.indexOf('=')+1)).map(([name,value])=>({name,value:value.slice(value.indexOf('=')+1),url:c.state.cookieOrigins[name],httpOnly:true,sameSite:'Lax'}));
 assert(cookies.length&&cookies.every(cookie=>Object.values(f.report.origins).includes(cookie.url)));await context.addCookies(cookies);
 const page=await context.newPage();page.setDefaultTimeout(20000);const errors=[];page.on('pageerror',error=>errors.push(error.message));
 await page.goto(`${f.report.origins.console}/workspaces/${f.report.customer}/projects/${f.report.project}/resources/archives/${c.state.resource}`);
 await page.getByRole('heading',{name:'Observed project report',exact:true}).waitFor();
 await page.getByRole('button',{name:'Download',exact:true}).click();const save=page.getByRole('link',{name:'Save project-report.csv.gz',exact:true});await save.waitFor();assert((await save.getAttribute('href')).startsWith('blob:'));
 const bytes=await page.evaluate(async()=>{const link=[...document.querySelectorAll('a')].find(a=>a.textContent==='Save project-report.csv.gz');const response=await fetch(link.href);return new Uint8Array(await response.arrayBuffer()).length;});assert.equal(bytes,f.report.dataProof.bytes);
 await page.getByRole('button',{name:'Hide download',exact:true}).click();await save.waitFor({state:'detached'});
 for(const width of [375,1280]){await page.setViewportSize({width,height:900});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,'archives_horizontal_overflow');const path=f.path('archives-console-'+width+'.png');await page.screenshot({path,fullPage:true});await chmod(path,0o600);}
 // Complete the custom Create -> Generate -> status -> Download -> explicit Delete flow in the actual UI.
 await page.goto(`${f.report.origins.console}/workspaces/${f.report.customer}/projects/${f.report.project}`);
 await page.getByText('Assign to this project',{exact:true}).click();
 await page.getByRole('button',{name:/Protected exports/}).click();
 await page.getByRole('heading',{name:'Create protected export',exact:true}).waitFor();
 await page.getByRole('combobox',{name:'Content',exact:true}).selectOption('report');
 await page.getByRole('button',{name:'Create export',exact:true}).click();
 await page.getByRole('button',{name:'Generate export',exact:true}).waitFor();
 const browserResource=new URL(page.url()).pathname.split('/').at(-1);assert(browserResource!==c.state.resource);
 await page.getByRole('button',{name:'Generate export',exact:true}).click();
 await page.getByRole('button',{name:'Download',exact:true}).waitFor({state:'visible'});
 await page.waitForFunction(()=>[...document.querySelectorAll('button')].some(b=>b.textContent==='Download'&&!b.disabled),null,{timeout:60000});
 await page.getByRole('button',{name:'Download',exact:true}).click();await page.getByRole('link',{name:'Save project-report.csv.gz',exact:true}).waitFor();
 await page.getByRole('button',{name:'Hide download',exact:true}).click();
 await page.getByRole('checkbox',{name:'Delete this export and stop any pending job',exact:true}).check();
 await page.getByRole('button',{name:'Delete export',exact:true}).click();
 await page.waitForURL(url=>!url.pathname.includes(browserResource));
 // The server accepts Generate, but its reply is lost. A later explicit Delete
 // supersedes that failed browser attempt without claiming frontend cancellation stops the job.
 const recoveryID=crypto.randomUUID();await invoke(c,f,'archives.create',{request_id:recoveryID,kind:'report',source_id:f.report.project,retention_seconds:3600},recoveryID);
 let generation, deletion;
 await page.route('**/api/console/plugins/invoke',async route=>{
  const body=route.request().postDataJSON();
  if(body.action==='archives.generate'&&body.input.resource_id===recoveryID&&!generation){const response=await route.fetch();assert.equal(response.status(),200);generation={body,result:await response.json()};await route.abort('failed');}
  else{if(body.action==='archives.delete'&&body.input.resource_id===recoveryID)deletion=body;await route.continue();}
 });
 await page.goto(`${f.report.origins.console}/workspaces/${f.report.customer}/projects/${f.report.project}/resources/archives/${recoveryID}`);
 await page.getByRole('button',{name:'Generate export',exact:true}).click();await page.getByRole('button',{name:'Retry generation',exact:true}).waitFor();
 assert(generation?.result.id,'actual_generation_was_accepted');
 await page.getByRole('button',{name:'Refresh status',exact:true}).click();
 await page.getByRole('checkbox',{name:'Delete this export and stop any pending job',exact:true}).check();
 await page.getByRole('button',{name:'Delete export',exact:true}).click();await page.waitForURL(url=>!url.pathname.includes(recoveryID));
 assert.equal(deletion?.action,'archives.delete');assert.equal(deletion?.input.resource_id,recoveryID);assert.notEqual(deletion?.idempotencyKey,generation.body.idempotencyKey);
 assert.equal(errors.length,0);await context.close();
 f.report.browserProof={realConsoleAndCore:true,actualOperatorPasswordTotpPkce:true,customNativeResourceUI:true,customCreateGenerateStatusDownloadDelete:true,acceptedGenerateReplyLost:true,deleteSupersedesFailedGenerateWithOwnIdentity:true,widths:[375,1280],knownSHAAuthenticatedBlobGET:true,blobBytesVerified:bytes,hideRetiresObjectURL:true,noHorizontalOverflow:true,pageErrors:0,screenshotsPrivate:true};await f.saveReport();console.log('native_archives_browser_passed '+f.input.runID);
}catch(error){await f.write('browser-failure-'+Date.now()+'.json',{error:String(error),stack:error.stack});process.stderr.write('native_archives_browser_failed_private_diagnostics_retained\n');process.exitCode=1;}
finally{await browser?.close();}
