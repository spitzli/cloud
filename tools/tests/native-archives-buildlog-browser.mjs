// Existing repository Playwright qualification; normal Native cookies, invitation and PKCE only.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {randomUUID} from 'node:crypto';
import {chmod} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {load} from './native-archives-fixture.mjs';
import {client,invoke} from './native-archives-acceptance.mjs';
import {sourceOperation} from './native-archives-buildlogs.mjs';
import {random} from './native-plugin-fixture.mjs';
const [runID,stage='read']=process.argv.slice(2);const f=await load(runID);let browser;
async function operatorContext(c){const context=await browser.newContext({viewport:{width:1280,height:900}});await context.addCookies(Object.entries(c.state.cookies).filter(([,v])=>v.slice(v.indexOf('=')+1)).map(([name,value])=>({name,value:value.slice(value.indexOf('=')+1),url:c.state.cookieOrigins[name],httpOnly:true,sameSite:'Lax'})));return context;}
async function reader(c){
 const account=existsSync(f.path('buildlog-reader-private.json'))?await f.json(f.path('buildlog-reader-private.json')):{email:`archives-reader-${runID}@example.test`,password:'Owned-'+random()+'!'};const {email,password}=account;
 const invite=account.invite??await c.json('/api/console/tenant/'+f.report.customer,'POST',{action:'invite',email,name:'Owned buildlog reader',role:'member'});assert(invite.link&&invite.id);if(!account.invite)await f.write('buildlog-reader-private.json',{email,password,invite});
 const context=await browser.newContext({viewport:{width:1280,height:900}});const token=new URL(invite.link).searchParams.get('token');assert(token);
 const postAuth=async(path,data)=>{const r=await context.request.post(f.report.origins.account+'/api/auth/'+path,{data,headers:{origin:f.report.origins.account}});assert.equal(r.status(),200);return r.json();};
 const activation=await context.request.post(f.report.origins.account+'/api/auth/reset-password',{data:{token,newPassword:password},headers:{origin:f.report.origins.account}});assert([200,401].includes(activation.status()));await postAuth('sign-in/email',{email,password});
 const invitation=await(await context.request.get(f.report.origins.account+'/api/invitation/'+invite.id)).json();if(invitation.status==='pending'){const accepted=await context.request.post(f.report.origins.account+'/api/invitation/'+invite.id,{data:{accept:true},headers:{origin:f.report.origins.account}});assert.equal(accepted.status(),200);}else assert.equal(invitation.status,'accepted');
 let url=f.report.origins.console+'/api/sso/login?returnTo=/projects';
 for(let n=0;n<12;n++){const r=await context.request.get(url,{maxRedirects:0});if(r.status()>=300&&r.status()<400){url=new URL(r.headers().location,url).href;continue;}if(r.headers()['content-type']?.includes('application/json')){const v=await r.json();if(v.redirect&&v.url){url=new URL(v.url,url).href;continue;}}assert.equal(r.status(),200);break;}
 const session=await(await context.request.get(f.report.origins.console+'/api/console/session')).json();assert.equal(session.data.user.role,'user');assert(!session.data.user.preview&&!session.data.user.acting);
 const project=await(await context.request.get(f.report.origins.console+'/api/console/project/'+f.report.project)).json();assert.equal(project.role,'member');
 const action=async(action,input)=>context.request.post(f.report.origins.console+'/api/console/plugins/invoke',{data:{pluginID:'archives',projectID:f.report.project,action,input,idempotencyKey:randomUUID()},headers:{origin:f.report.origins.console}});
 const choices=await action('archives.sources',{});assert([200,403].includes(choices.status()));if(choices.status()===200)assert(!(await choices.json()).sources.some(s=>s.kind==='buildlog'));
 const status=await action('archives.status',{resource_id:c.state.buildlog.succeeded.resource});assert.equal(status.status(),200,'reader retains ordinary resource metadata access');
 const denied=await action('archives.download',{resource_id:c.state.buildlog.succeeded.resource});assert([403,409].includes(denied.status()));const rejected=await denied.json();assert(!rejected.path&&!rejected.attachment,'denial must not expose a transfer descriptor');
 f.report.buildlogReaderProof={realInvitationActivationPkce:true,actualMember:true,ordinaryProjectAndArchiveStatusAllowed:true,buildlogSourcesNotDisclosed:true,downloadDeniedWithoutDeploymentsCreate:true,downloadDenialStatus:denied.status()};await f.saveReport();await context.close();
}
try {
 assert(f.report.buildlogNativeProof?.actualCoreCompletionToNativeJobS3Gateway);const c=await client(f);
 browser=await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE?{executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE}:{})});
 const context=await operatorContext(c);const page=await context.newPage();page.setDefaultTimeout(20000);const errors=[];page.on('pageerror',e=>errors.push(e.message));
 if(stage==='read'){
  for(const outcome of ['succeeded','failed']){const output=c.state.buildlog[outcome];await page.goto(`${f.report.origins.console}/workspaces/${f.report.customer}/projects/${f.report.project}/resources/archives/${output.resource}`);await page.getByText(/Actual stored Core build output, potentially sensitive/).waitFor();assert((await page.locator('body').innerText()).includes('potentially sensitive'));assert((await page.locator('body').innerText()).includes('may be truncated'));await page.getByRole('button',{name:'Download',exact:true}).click();const save=page.getByRole('link',{name:'Save build.log.gz',exact:true});await save.waitFor();
   const proof=await page.evaluate(async()=>{const link=[...document.querySelectorAll('a')].find(a=>a.textContent==='Save build.log.gz');const bytes=await(await fetch(link.href)).arrayBuffer();const digest=await crypto.subtle.digest('SHA-256',bytes);return {bytes:bytes.byteLength,sha256:[...new Uint8Array(digest)].map(v=>v.toString(16).padStart(2,'0')).join('')};});assert.equal(proof.bytes,output.bytes);assert.equal(proof.sha256,output.sha256);assert(!(await page.locator('body').innerText()).includes('actual-native-buildlog-'));assert.equal(await page.locator('pre,textarea,iframe').count(),0);await page.getByRole('button',{name:'Hide download',exact:true}).click();}
  for(const width of [375,1280]){await page.setViewportSize({width,height:900});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);const file=f.path('buildlog-console-'+width+'.png');await page.screenshot({path:file,fullPage:true});await chmod(file,0o600);}
  await reader(c);f.report.buildlogBrowserProof={actualNativeCookies:true,succeededAndFailedSources:true,knownShaBlob:true,noRawPreview:true,honestSensitivityAndTruncationNotice:true,widths:[375,1280],pageErrors:errors.length};
 }else if(stage==='disconnect'){
  const pending=randomUUID();await invoke(c,f,'archives.create',{request_id:pending,kind:'buildlog',source_id:f.report.buildlog.sources.succeeded.buildID,retention_seconds:3600},pending);
  c.state.buildlog.succeeded.descriptor=await invoke(c,f,'archives.download',{resource_id:c.state.buildlog.succeeded.resource});await c.save();const before=await c.raw(f.report.origins.console+c.state.buildlog.succeeded.descriptor.path);assert.equal(before.status,200);await before.arrayBuffer();
  await page.goto(`${f.report.origins.console}/workspaces/${f.report.customer}/projects/${f.report.project}/resources/archives/${pending}`);await page.getByText(/Actual stored Core build output, potentially sensitive/).waitFor();await sourceOperation(f,'disconnect');
  await page.getByRole('button',{name:'Generate export',exact:true}).click();await page.getByRole('button',{name:'Retry generation',exact:true}).waitFor();await page.getByRole('button',{name:'Refresh status',exact:true}).click();await page.getByRole('checkbox',{name:'Delete this export and stop any pending job',exact:true}).check();await page.getByRole('button',{name:'Delete export',exact:true}).click();await page.waitForURL(url=>!url.pathname.includes(pending));
  const response=await c.raw(f.report.origins.console+c.state.buildlog.succeeded.descriptor.path);assert([401,403,404,409].includes(response.status));await response.arrayBuffer();await invoke(c,f,'archives.delete',{resource_id:c.state.buildlog.succeeded.resource});
  f.report.buildlogDisconnectProof={actualCoreGitDisconnect:true,oldOpaqueDescriptorDenied:true,actualFailedGenerateThenRefreshDelete:true,exactDeletionAfterSourceRevocation:true,pageErrors:errors.length};
 }else throw Error('unsupported_buildlog_browser_stage');
 assert.equal(errors.length,0);await context.close();await f.saveReport();console.log('native_archives_buildlog_browser_'+stage+'_passed '+runID);
}catch(error){await f.write('buildlog-browser-failure-'+Date.now()+'.json',{stage,error:String(error),stack:error.stack});process.stderr.write('native_archives_buildlog_browser_failed_private_diagnostics_retained\n');process.exitCode=1;}
finally{await browser?.close();}
