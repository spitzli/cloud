// Owned local developer fixture only. Never source deployment configuration here.
import { readFileSync, writeFileSync, readlinkSync, openSync } from 'node:fs';
import { parseEnv } from 'node:util';
import { spawn } from 'node:child_process';
import { accountFixture } from '../../../apps/console/tests/account-fixture.mjs';
const e=parseEnv(readFileSync('.runtime-test/identity-restricted.env','utf8'));
for(const key of ['AUTH_DATABASE_URL','DATABASE_URL','WEBDOCK_REGISTRY_DATABASE_URL']){const u=new URL(e[key]);if(u.hostname!=='127.0.0.1'||u.pathname!=='/webdock_rls_test'||u.search||u.hash)throw Error('Owned loopback test database required');}
const fixture=JSON.parse(readFileSync(accountFixture,'utf8'));
try { const pid=Number(readFileSync('.runtime-test/api.pid','utf8'));if(!readlinkSync(`/proc/${pid}/exe`).startsWith(process.cwd()+'/target/debug/webdock-control-plane'))throw Error('Unexpected API process');process.kill(pid,'SIGTERM');for(let i=0;i<40;i++){try{process.kill(pid,0);await new Promise(resolve=>setTimeout(resolve,50));}catch{break;}} }catch(error){if(error.code!=='ENOENT'&&error.code!=='ESRCH')throw error;}
const env={PATH:process.env.PATH,HOME:process.env.HOME,NODE_ENV:'development',WEBDOCK_DATABASE_URL:e.AUTH_DATABASE_URL,WEBDOCK_SESSION_DATABASE_URL:e.DATABASE_URL,WEBDOCK_REGISTRY_DATABASE_URL:e.WEBDOCK_REGISTRY_DATABASE_URL,WEBDOCK_AUTH_SECRET:e.BETTER_AUTH_SECRET,DATABASE_RLS_SECRET:e.DATABASE_RLS_SECRET,WEBDOCK_API_ORIGIN:'http://127.0.0.1:3130',WEBDOCK_AUTH_ORIGIN:'http://localhost:3125',WEBDOCK_STUDIO_ORIGIN:'http://127.0.0.1:3120',WEBDOCK_CONSOLE_ORIGIN:'http://127.0.0.1:3121',WEBDOCK_STUDIO_CLIENT_ID:fixture.clientID,WEBDOCK_SSO_CLIENT_ID:fixture.clientID,WEBDOCK_SSO_CLIENT_SECRET:fixture.clientSecret,WEBDOCK_SSO_COOKIE_SECRET:fixture.cookieSecret};
if(process.env.WEBDOCK_BROWSER_DATABASES==='true'){
  const database=JSON.parse(readFileSync('.runtime-test/database-browser.json','utf8'));
  const worker=new URL(e.WEBDOCK_PLUGIN_WORKER_DATABASE_URL);if(worker.hostname!=='127.0.0.1'||worker.pathname!=='/webdock_rls_test'||worker.search||worker.hash)throw Error('Owned worker fixture required');
  if(process.env.WEBDOCK_BROWSER_MANAGEMENT==='true'){const managed=JSON.parse(readFileSync('.runtime-test/database-management-browser.json','utf8'));env.WEBDOCK_DATABASE_CLUSTER_ID=managed.clusterID;env.WEBDOCK_DATABASE_DOMAIN='database.example.invalid';}
  env.WEBDOCK_WORKER_DATABASE_URL=worker.toString();env.WEBDOCK_FEATURES=JSON.stringify({databases:'enabled'});env.WEBDOCK_DATABASE_GATEWAY_ORIGIN=database.gatewayOrigin;env.WEBDOCK_DATABASE_GATEWAY_SECRET=database.gatewaySecret;
}
if(process.env.WEBDOCK_BROWSER_STATIC==='true'){env.WEBDOCK_UI_DIST=process.cwd()+'/apps/console/dist';env.WEBDOCK_UI_CONNECT_ORIGINS=JSON.stringify(['http://localhost:3125',...(process.env.WEBDOCK_BROWSER_DATABASES==='true'?['http://127.0.0.1:3150']:[])]);}
const log=openSync('.runtime-test/account-api.log','a',0o600);const child=spawn('./target/debug/webdock-control-plane',[],{env,stdio:['ignore',log,log],detached:true});child.unref();writeFileSync('.runtime-test/api.pid',String(child.pid),{mode:0o600});console.log('Owned local account API started.');
