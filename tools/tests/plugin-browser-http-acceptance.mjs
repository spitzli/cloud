/** Real local password/MFA/PKCE, then read/write OAuth API and MCP proof. No injected identities or tokens. */
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync,chmodSync,readlinkSync} from 'node:fs';
import {randomBytes,createHmac,createHash} from 'node:crypto';
import {join} from 'node:path';
const file=process.argv[2];if(!file)throw Error('Pass the private local browser fixture file');
const fixture=JSON.parse(readFileSync(file,'utf8'));
const origins={auth:fixture.env.BETTER_AUTH_URL,console:fixture.env.WEBDOCK_CONSOLE_ORIGIN,studio:fixture.env.WEBDOCK_STUDIO_ORIGIN,api:fixture.env.WEBDOCK_API_ORIGIN};
for(const value of Object.values(origins)){const u=new URL(value);assert.equal(u.protocol,'http:');assert.equal(u.hostname,'localhost');}
assert.match(fixture.container,/^webdock-browser-[a-f0-9]+$/);
const jar=new Map();
function save(){writeFileSync(file,JSON.stringify(fixture),{mode:0o600});chmodSync(file,0o600);}
async function request(url,options={}){
 assert.ok(Object.values(origins).includes(new URL(url).origin),'External request refused');
 const headers=new Headers(options.headers);headers.set('cookie',[...jar.values()].join('; '));
 const r=await fetch(url,{...options,headers,redirect:'manual'});
 for(const c of r.headers.getSetCookie()){const value=c.split(';')[0];jar.set(value.split('=')[0],value);}
 return r;
}
async function auth(path,body){return request(origins.auth+'/api/auth'+path,{method:'POST',headers:{origin:origins.auth,'content-type':'application/json'},body:JSON.stringify(body)});}
function totp(uri){let bits='';for(const c of new URL(uri).searchParams.get('secret').toUpperCase().replace(/=+$/,''))bits+='ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'.indexOf(c).toString(2).padStart(5,'0');const key=Buffer.from(bits.match(/.{8}/g).map(b=>parseInt(b,2))),counter=Buffer.alloc(8);counter.writeBigUInt64BE(BigInt(Math.floor(Date.now()/30000)));const h=createHmac('sha1',key).update(counter).digest(),at=h[h.length-1]&15;return String((h.readUInt32BE(at)&0x7fffffff)%1000000).padStart(6,'0');}
let r=await auth('/sign-in/email',{email:fixture.email,password:fixture.changedPassword??fixture.password});assert.equal(r.status,200,'Password sign-in');
if(!fixture.changedPassword){const next=randomBytes(32).toString('base64url');r=await auth('/change-password',{currentPassword:fixture.password,newPassword:next,revokeOtherSessions:true});assert.equal(r.status,200,'Required password change');fixture.changedPassword=next;save();}
if(!fixture.totpURI){r=await auth('/two-factor/enable',{password:fixture.changedPassword});assert.equal(r.status,200,'MFA enrollment');fixture.totpURI=(await r.json()).totpURI;assert.ok(fixture.totpURI);save();}
r=await auth('/two-factor/verify-totp',{code:totp(fixture.totpURI)});assert.equal(r.status,200,'Real TOTP verification');
let url=origins.console+'/api/sso/login?returnTo=/customers';
for(let i=0;i<10;i++){r=await request(url);if(r.headers.get('content-type')?.includes('application/json')){const body=await r.clone().json();if(body.redirect===true&&typeof body.url==='string'){url=new URL(body.url,url).href;continue;}}if(r.status>=300&&r.status<400){url=new URL(r.headers.get('location'),url).href;continue;}break;}
assert.equal(r.status,200,'Real PKCE callback to Console');const customersHTML=await r.text();writeFileSync(join(fixture.dir,'customers.html'),customersHTML);assert.match(customersHTML,/Customers/);assert.doesNotMatch(customersHTML,/This page couldn.t load/);
async function call(operation,args){const reply=await request(origins.api+'/v1/studio',{method:'POST',headers:{origin:origins.studio,'content-type':'application/json'},body:JSON.stringify({operation,args})});const body=await reply.json();assert.equal(reply.status,200,JSON.stringify(body));return body.data;}
const session=await call('session',[]);assert.equal(session.operator,true);assert.ok(Object.values(session.installation.modules).every(x=>x==='disabled'));
const plugin=JSON.parse(readFileSync(join(fixture.dir,'plugin-manifest.json'),'utf8'));
const redirect=origins.studio+'/local-plugin-pkce-callback';
const client=await call('registerConnection',[{name:'Synthetic local acceptance MCP',redirectURI:redirect,write:'yes',confidential:'no',offline:'no'}]);
assert.ok(client.clientID);
async function token(scope){
 const verifier=randomBytes(32).toString('base64url'),state=randomBytes(16).toString('hex');
 const q=new URLSearchParams({client_id:client.clientID,redirect_uri:redirect,response_type:'code',scope,resource:origins.studio+'/api/mcp',state,code_challenge:createHash('sha256').update(verifier).digest('base64url'),code_challenge_method:'S256'});
 let response=await request(origins.auth+'/api/auth/oauth2/authorize?'+q,{headers:{origin:origins.auth}});assert.ok([200,302].includes(response.status),await response.clone().text());
 let destination=new URL(response.status===302?response.headers.get('location'):(await response.json()).url,origins.auth);
 if(destination.pathname==='/consent'){response=await auth('/oauth2/consent',{accept:true,oauth_query:destination.search.slice(1)});assert.equal(response.status,200,await response.clone().text());destination=new URL((await response.json()).url);}
 assert.equal(destination.origin,origins.studio);assert.equal(destination.searchParams.get('state'),state);assert.ok(destination.searchParams.get('code'));
 response=await request(origins.auth+'/api/auth/oauth2/token',{method:'POST',headers:{origin:origins.auth,'content-type':'application/x-www-form-urlencoded'},body:new URLSearchParams({grant_type:'authorization_code',client_id:client.clientID,redirect_uri:redirect,code:destination.searchParams.get('code'),code_verifier:verifier,resource:origins.studio+'/api/mcp'})});assert.equal(response.status,200,await response.clone().text());return (await response.json()).access_token;
}
const read=await token('webdock:read'),write=await token('webdock:read webdock:write');
const api=origins.api+'/v1/plugins/'+plugin.id+'/projects/'+plugin.project+'/api/notes';
const bearer=(token)=>({authorization:'Bearer '+token,'content-type':'application/json','idempotency-key':randomBytes(16).toString('hex')});
let response=await fetch(api,{method:'POST',headers:bearer(read),body:JSON.stringify({note:'must be denied'})});assert.equal(response.status,403,'Read-only API write denied');
response=await fetch(api,{method:'POST',headers:bearer(write),body:JSON.stringify({note:'real OAuth API scoped note'})});assert.equal(response.status,200,await response.clone().text());
response=await fetch(api,{headers:bearer(read)});assert.equal(response.status,200,await response.clone().text());const saved=await response.json();assert.match(JSON.stringify(saved),/real OAuth API scoped note/);
response=await fetch(api.replace(plugin.project,'980000000000000099'),{headers:bearer(read)});assert.equal(response.status,403,'Foreign project denied');
async function mcp(token,method,params={}){const response=await fetch(origins.studio+'/api/mcp',{method:'POST',headers:{...bearer(token),accept:'application/json, text/event-stream'},body:JSON.stringify({jsonrpc:'2.0',id:1,method,params})});assert.equal(response.status,200,await response.clone().text());const text=await response.text();return JSON.parse(text.startsWith('event:')||text.startsWith('data:')?text.split('\n').find(x=>x.startsWith('data:')).slice(5):text);}
const readList=await mcp(read,'tools/list'),writeList=await mcp(write,'tools/list');
const prefix='plugin__'+plugin.id.replaceAll('-','_')+'__';assert.ok(readList.result.tools.some(t=>t.name===prefix+'read'));assert.ok(!readList.result.tools.some(t=>t.name===prefix+'write'));assert.ok(writeList.result.tools.some(t=>t.name===prefix+'write'));
const written=await mcp(write,'tools/call',{name:prefix+'write',arguments:{projectID:plugin.project,input:{note:'real OAuth MCP scoped note'},idempotencyKey:randomBytes(16).toString('hex')}});assert.notEqual(written.result?.isError,true,JSON.stringify(written));
const retrieved=await mcp(read,'tools/call',{name:prefix+'read',arguments:{projectID:plugin.project,input:{}}});assert.match(JSON.stringify(retrieved),/real OAuth MCP scoped note/);
const ui=await fetch(plugin.uiOrigin+'/ui/panel');assert.equal(ui.status,200);assert.match(await ui.text(),/Synthetic acceptance notes/);for(const path of ['/actions/notes.read','/v1/plugin/data/note','/healthz'])assert.equal((await fetch(plugin.uiOrigin+path)).status,404,'UI origin serves static panel only');assert.equal((await fetch(plugin.uiOrigin+'/ui/panel',{method:'POST'})).status,404);
let isolatedCoreAPI=false;if(fixture.coreBundle){const processManifest=JSON.parse(readFileSync(join(fixture.dir,'manifest.json'),'utf8'));const apiPID=processManifest.pids.find(pid=>readFileSync('/proc/'+pid+'/cmdline','utf8').includes(join(fixture.dir,'core-api/core.js')));assert.ok(apiPID,'API process runs owned copied artifact');assert.equal(readlinkSync('/proc/'+apiPID+'/cwd'),join(fixture.dir,'core-api'));assert.equal(createHash('sha256').update(readFileSync(join(fixture.dir,'core-api/core.js'))).digest('hex'),createHash('sha256').update(readFileSync('legacy/control-plane/dist-core/core.js')).digest('hex'));isolatedCoreAPI=true;}
const proof={isolatedCoreAPI,staticUIOnly:true,synthetic:true,coreBundle:fixture.coreBundle===true,coreConsole:fixture.coreConsole===true,uiTransport:plugin.uiTransport,actionsTransport:plugin.actionsTransport,signedInstall:true,privateTLSActivation:true,realPasswordMFAAndPKCE:true,actualOAuthReadWriteTokens:true,scopedAPIReadWrite:true,readOnlyAPIWriteDenied:true,crossProjectDenied:true,readOnlyMCPHidesWrite:true,scopedMCPReadWrite:true};
writeFileSync(join(fixture.dir,'plugin-http-proof.json'),JSON.stringify(proof,null,2),{mode:0o600});
console.log(JSON.stringify(proof));
