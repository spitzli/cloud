import assert from 'node:assert/strict';
import {readFileSync,writeFileSync,chmodSync} from 'node:fs';
import {randomBytes,createHmac} from 'node:crypto';
import {join} from 'node:path';
const file=process.argv[2];if(!file)throw Error('Pass the private local browser fixture file');
const fixture=JSON.parse(readFileSync(file,'utf8'));
const origins={auth:fixture.env.BETTER_AUTH_URL,console:fixture.env.SPITZLI_CONSOLE_ORIGIN,studio:fixture.env.SPITZLI_STUDIO_ORIGIN,api:fixture.env.SPITZLI_API_ORIGIN};
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
const customer=await call('registry.customer',[null,{name:'Local acceptance workspace'}]);assert.ok(customer.id);
const project=await call('registry.project',[null,{name:'Local acceptance project',customer:customer.id}]);assert.ok(project.id);
for(const path of ['/projects/'+project.id,'/plugins/'+project.id]){const page=await request(origins.console+path);assert.equal(page.status,200,path);const html=await page.text();assert.doesNotMatch(html,/This page couldn.t load|NEXT_HTTP_ERROR_FALLBACK;500/);writeFileSync(join(fixture.dir,path.startsWith('/plugins')?'plugins.html':'project.html'),html);}
for(const path of ['/sites','/admin/billing']){const page=await request(origins.console+path);assert.equal(page.status,404,path);}
const crossOrigin=await request(origins.api+'/v1/studio',{method:'POST',headers:{origin:'https://foreign.invalid','content-type':'application/json'},body:JSON.stringify({operation:'registry.customer',args:[null,{name:'Must not be created'}]})});
assert.equal(crossOrigin.status,403,'Cross-origin write denied');
assert.equal((await auth('/sign-out',{})).status,200,'Real logout');
const revoked=await request(origins.api+'/v1/studio',{method:'POST',headers:{origin:origins.studio,'content-type':'application/json'},body:JSON.stringify({operation:'session',args:[]})});
assert.equal(revoked.status,401,'Previously issued Console session revoked');
fixture.proof={realHTTP:true,realPasswordChange:true,realTOTP:true,realPKCE:true,authenticatedConsoleSSR:true,customerID:customer.id,projectID:project.id,pluginPanelSSR:true,allProductsDisabled:true,removedCMSAndBilling404:true,crossOriginWriteDenied:true,logoutRevokesConsoleSession:true};save();
console.log(JSON.stringify(fixture.proof));
