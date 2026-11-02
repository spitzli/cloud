import {randomBytes} from 'node:crypto';
import {execFileSync,spawnSync,spawn} from 'node:child_process';
import {mkdtemp,writeFile,open,mkdir,chmod,readFile,copyFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createServer} from 'node:net';
import {Pool} from 'pg';
import {runtimeRoles} from '../migrations/config';
import {ownedCleanup} from './core-browser-cleanup.mjs';
import {initializePluginState} from '../../legacy/control-plane/src/modules/plugins/bootstrap';
const root=process.cwd(), dir=await mkdtemp(join(tmpdir(),'webdock-core-browser-'));
for(const port of [...(process.env.WEBDOCK_BROWSER_PLUGINS==='true'?[3443,3444,3446]:[]),3460,3461,3463,3465,3470,3475]) await new Promise<void>((ok,bad)=>{const s=createServer();s.once('error',bad);s.listen(port,'127.0.0.1',()=>s.close(()=>ok()));});
const pluginMode=process.env.WEBDOCK_BROWSER_PLUGINS==='true',coreConsole=process.env.WEBDOCK_BROWSER_CORE_CONSOLE==='true';
const clerkMode=process.env.WEBDOCK_BROWSER_CLERK==='true';
if(clerkMode&&(!process.env.WEBDOCK_BROWSER_CLERK_CREDENTIALS||!process.env.WEBDOCK_BROWSER_CLERK_DEV_ENV))throw Error('Explicit private Clerk credentials and dedicated development env paths required');
if(pluginMode){await mkdir(join(dir,'tls'),{mode:0o700});execFileSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-keyout',join(dir,'tls/key.pem'),'-out',join(dir,'tls/cert.pem'),'-days','1','-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost,IP:127.0.0.1,DNS:fixture.plugin-browser.svc.cluster.local'],{stdio:'ignore'});await chmod(join(dir,'tls/key.pem'),0o600);}
const endpoint=execFileSync('docker',['context','inspect','--format','{{.Endpoints.docker.Host}}'],{encoding:'utf8'}).trim();
if(!endpoint.startsWith('unix://'))throw Error('Local Docker required');
const dockerEnv={PATH:process.env.PATH,HOME:process.env.HOME,DOCKER_HOST:endpoint};
const docker=(args:string[])=>execFileSync('docker',args,{env:dockerEnv,encoding:'utf8'}).trim();
const name='webdock-browser-'+randomBytes(6).toString('hex'), password=randomBytes(32).toString('hex');
docker(['run','-d','--name',name,'--label','webdock.browser-fixture='+name,'-e','POSTGRES_PASSWORD='+password,'-e','POSTGRES_DB=webdock_operator_test','-p','127.0.0.1::5432',...(pluginMode?['-v',join(dir,'tls')+':/fixture:ro']:[]),'postgres:18-alpine',...(pluginMode?['sh','-c','cp /fixture/key.pem /tmp/server.key && cp /fixture/cert.pem /tmp/server.crt && chown postgres:postgres /tmp/server.key /tmp/server.crt && chmod 600 /tmp/server.key && exec docker-entrypoint.sh postgres -c ssl=on -c ssl_key_file=/tmp/server.key -c ssl_cert_file=/tmp/server.crt']:[])]);
let owner:Pool|undefined, poolClosed=false, stopping=false;
const children:any[]=[], proxies:any[]=[];
const cleanup=ownedCleanup({name,
 closeServers:async()=>{stopping=true;for(const p of proxies)p.stop(true);},
 closePool:async()=>{if(owner&&!poolClosed){await owner.end();poolClosed=true;}},
 stopChildren:async()=>{await Promise.all(children.map(async c=>{if(c.exitCode!==null||c.signalCode!==null)return;const ended=new Promise<void>(resolve=>c.once('exit',()=>resolve()));c.kill('SIGTERM');const timer=setTimeout(()=>c.kill('SIGKILL'),5000);try{await ended;}finally{clearTimeout(timer);}}));},
 inspectLabel:async()=>docker(['inspect','--format','{{index .Config.Labels "webdock.browser-fixture"}}',name]),
 removeContainer:async()=>{docker(['rm','-f',name]);}
});
const stop=(failed=false)=>{void cleanup().then(()=>process.exit(failed?1:0),()=>process.exit(1));};
process.once('SIGTERM',()=>stop());process.once('SIGINT',()=>stop());
try {
const port=docker(['port',name,'5432/tcp']).split(':').at(-1)!;
const url=new URL(`postgresql://postgres:${password}@127.0.0.1:${port}/webdock_operator_test`);owner=new Pool({connectionString:url.href,max:1});
for(let n=0;;n++){try{await owner.query('SELECT 1');break;}catch{if(n>60)throw Error('DB unavailable');await new Promise(r=>setTimeout(r,250));}}
const passwords=Object.fromEntries([...runtimeRoles,'webdock_offline_migrate'].map(r=>[r,randomBytes(32).toString('hex')]));
const base={PATH:process.env.PATH!,HOME:process.env.HOME!,NODE_ENV:'development',DATABASE_URL_UNPOOLED:url.href,DATABASE_CONTEXT_SECRET:randomBytes(48).toString('hex'),BETTER_AUTH_URL:'http://localhost:3465',WEBDOCK_STUDIO_ORIGIN:'http://localhost:3461',WEBDOCK_CONSOLE_ORIGIN:'http://localhost:3460'};
const target=`127.0.0.1:${port}/webdock_operator_test`;
const run=(file:string,args:string[],env:object)=>{const r=spawnSync(process.execPath,['--no-env-file','tools/migrations/'+file,target,...args],{cwd:root,env:{...base,...env},encoding:'utf8'});if(r.status!==0)throw Error(file+' failed: '+r.stderr);};
run('install.ts',[],{WEBDOCK_BOOTSTRAP_EMPTY_DATABASE:'true',MIGRATION_ROLE_PASSWORDS:JSON.stringify(passwords)});
url.username='webdock_offline_migrate';url.password=passwords.webdock_offline_migrate;
const operatorPassword=randomBytes(24).toString('base64url'),input=join(dir,'input.json'),output=join(dir,'operator.json');
await writeFile(input,JSON.stringify({email:'operator@example.test',name:'Browser fixture operator',password:operatorPassword,authOrigin:base.BETTER_AUTH_URL,studioOrigin:base.WEBDOCK_STUDIO_ORIGIN,consoleOrigin:base.WEBDOCK_CONSOLE_ORIGIN}),{mode:0o600});
run('operator.ts',[input,output],{DATABASE_URL_UNPOOLED:url.href});
const secrets=await Bun.file(output).json();
await initializePluginState(owner,{configVersion:1,modules:{}},'fresh');
const roleURL=(role:string)=>{const u=new URL(url);u.username=role;u.password=passwords[role];return u.href;};
const env={...base,...secrets.environment,PORT:'3463',BIND_ADDRESS:'127.0.0.1',BETTER_AUTH_SECRET:randomBytes(48).toString('hex'),DATABASE_URL:roleURL('webdock_auth_runtime'),AUTH_DATABASE_URL:roleURL('webdock_auth_bootstrap'),DATABASE_WORKER_URL:roleURL('webdock_worker_runtime'),REGISTRY_DATABASE_URL:roleURL('webdock_registry_runtime'),DATABASE_POOL_MAX:'2',REGISTRY_POOL_MAX:'2',DATABASE_RLS_SECRET:base.DATABASE_CONTEXT_SECRET,WEBDOCK_API_ORIGIN:'http://localhost:3463',...(pluginMode?{WEBDOCK_PLUGIN_UI_ALLOW_LOCAL_HTTP:'true'}:{}),WEBDOCK_API_DOMAINS:pluginMode?'auth,portal,registry,mcp':'auth,portal,registry',WEBDOCK_MCP_RESOURCE:'http://localhost:3461/api/mcp',WEBDOCK_AUTH_ISSUER:'http://localhost:3465/api/auth',WEBDOCK_MODULES:JSON.stringify({configVersion:1,modules:{}}),WEBDOCK_INGRESS_SECRET:randomBytes(48).toString('hex'),WEBDOCK_TRUSTED_PROXIES:'127.0.0.1/32',AUTH_TEST_MAIL:'true',WEBDOCK_SSO_ALLOW_LOCAL_HTTP:'true',NEXT_PUBLIC_AUTH_URL:base.BETTER_AUTH_URL,NEXT_PUBLIC_API_URL:'http://localhost:3463',NEXT_PUBLIC_WEBDOCK_API_ORIGIN:'http://localhost:3463'};
if(pluginMode){await (await import('./plugin-browser-setup')).preparePluginBrowser({dir,root,owner,secrets,env,offlineURL:url.href,target,children});}
if(pluginMode&&process.env.WEBDOCK_BROWSER_PLUGIN_SOURCES==='true')env.WEBDOCK_PLUGIN_INSTALLER_TOKEN=randomBytes(32).toString('hex');
let browserCredentials:any={email:'operator@example.test',password:operatorPassword};
if(clerkMode)browserCredentials=await (await import('./clerk-browser-setup')).prepareClerkBrowser({dir,owner,secrets,env,credentialsPath:process.env.WEBDOCK_BROWSER_CLERK_CREDENTIALS!,developmentEnvPath:process.env.WEBDOCK_BROWSER_CLERK_DEV_ENV!});
await owner.end();poolClosed=true;
await writeFile(join(dir,'private.json'),JSON.stringify({...browserCredentials,env,container:name,dir,coreBundle:pluginMode&&process.env.WEBDOCK_BROWSER_CORE_BUNDLE==='true',coreConsole},null,2),{mode:0o600});
async function child(label:string,command:string,args:string[],cwd=root){const log=await open(join(dir,label+'.log'),'a',0o600);const c=spawn(command,args,{cwd,env:label==='api'?env:{...env,NODE_OPTIONS:'--require='+JSON.stringify(root+'/tools/tests/core-browser-no-env.cjs')},stdio:['ignore',log.fd,log.fd]});children.push(c);c.once('exit',()=>{if(!stopping)stop(true);});const started=new Promise<void>((resolve,reject)=>{c.once('spawn',resolve);c.once('error',reject);});await Promise.all([log.close(),started]);return c;}
const bundle=pluginMode&&process.env.WEBDOCK_BROWSER_CORE_BUNDLE==='true';
if(bundle){await mkdir(join(dir,'core-api'),{mode:0o700});await copyFile(join(root,'legacy/control-plane/dist-core/core.js'),join(dir,'core-api/core.js'));}
await child('api',process.execPath,[...(process.env.WEBDOCK_BROWSER_PLUGIN_SOURCES==='true'?['--watch']:[]),'--no-env-file',bundle?join(dir,'core-api/core.js'):pluginMode?'legacy/control-plane/src/core.ts':'legacy/control-plane/src/index.ts'],bundle?join(dir,'core-api'):root);
for(let n=0;;n++){try{const ready=await fetch('http://localhost:3463/readyz');if(ready.ok)break;}catch{}if(n>=100)throw Error('Fixture API readiness timed out');await new Promise(resolve=>setTimeout(resolve,100));}
if(!clerkMode)await child('auth',process.env.WEBDOCK_FIXTURE_NODE||'node',[root+'/node_modules/next/dist/bin/next','dev','--hostname','127.0.0.1','--port','3475'],root+'/legacy/auth');
await child('console',process.env.WEBDOCK_FIXTURE_NODE||'node',[root+'/node_modules/next/dist/bin/next','dev',...(coreConsole?['--webpack']:[]),'--hostname','127.0.0.1','--port','3470'],coreConsole?root+'/.core-build/console':root+'/legacy/console');
proxies.push(...(await import('./core-browser-proxy.mjs')).browserProxies(env.WEBDOCK_INGRESS_SECRET));
await writeFile(join(dir,'cleanup.sh'),`#!/bin/sh\nkill -TERM ${process.pid} 2>/dev/null || true\nn=0\nwhile kill -0 ${process.pid} 2>/dev/null && [ "$n" -lt 10 ]; do sleep 1; n=$((n+1)); done\nlabel=$(DOCKER_HOST='${endpoint}' docker inspect --format '{{index .Config.Labels "webdock.browser-fixture"}}' '${name}' 2>/dev/null) || exit 0\n[ "$label" = '${name}' ] || { echo 'Refusing unowned container cleanup' >&2; exit 1; }\nDOCKER_HOST='${endpoint}' docker rm -f '${name}' >/dev/null\n`,{mode:0o700});
await writeFile(join(dir,'manifest.json'),JSON.stringify({dir,container:name,pid:process.pid,pids:children.map(c=>c.pid),credentials:join(dir,'private.json'),cleanup:join(dir,'cleanup.sh')}),{mode:0o600});
console.log(dir);
} catch(error) {
 try {await cleanup();} catch(cleanupError) {throw new AggregateError([error,cleanupError],'Fixture startup and cleanup failed');}
 throw error;
}
