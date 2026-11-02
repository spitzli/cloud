// Starts only owned local Rust runtime and gateway processes with synthetic credentials.
import {readFileSync,writeFileSync,mkdirSync,openSync,readlinkSync,realpathSync} from 'node:fs';
import {spawn,execFileSync} from 'node:child_process';
import {resolve} from 'node:path';
import https from 'node:https';
const fixture=JSON.parse(readFileSync('.runtime-test/database-browser.json','utf8'));
if(fixture.runtimeOrigin!=='https://localhost:3152'||fixture.gatewayOrigin!=='http://127.0.0.1:3150')throw Error('Unexpected local fixture');
const dir=resolve('.runtime-test/database-browser-runtime');mkdirSync(dir,{mode:0o700,recursive:true});
execFileSync('python3',['-c',`import sqlite3,sys
c=sqlite3.connect(sys.argv[1]);c.execute('CREATE TABLE IF NOT EXISTS orders(id INTEGER PRIMARY KEY,customer TEXT,total INTEGER)');c.execute("INSERT OR IGNORE INTO orders VALUES(1,'Local fixture',42)");c.commit()`,dir+'/orders.sqlite']);
writeFileSync(dir+'/connections.json',JSON.stringify([{id:'fixture-orders',name:'Orders',params:{driver:'sqlite',database:'orders.sqlite',save_in_keychain:false}}]),{mode:0o600});
writeFileSync(dir+'/config.json',JSON.stringify({checkForUpdates:false,autoCheckUpdatesOnStartup:false}),{mode:0o600});
async function start(label,executable,args,env){
 const pidFile=`.runtime-test/database-${label}.pid`;
 try{const pid=Number(readFileSync(pidFile,'utf8'));if(readlinkSync(`/proc/${pid}/exe`)!==realpathSync(executable))throw Error('Unexpected fixture process');process.kill(pid,'SIGTERM');await new Promise(r=>setTimeout(r,250));}catch(error){if(!['ENOENT','ESRCH'].includes(error.code))throw error;}
 const log=openSync(`.runtime-test/database-${label}.log`,'a',0o600);const child=spawn(executable,args,{env,cwd:label==='runtime'?dir:process.cwd(),stdio:['ignore',log,log],detached:true});child.unref();writeFileSync(pidFile,String(child.pid),{mode:0o600});
}
const cert=dir+'/server.crt', key=dir+'/server.key', ca=dir+'/ca.crt';
execFileSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-keyout',dir+'/ca.key','-out',ca,'-subj','/CN=owned-database-browser-ca','-days','1'],{stdio:'ignore'});
execFileSync('openssl',['req','-new','-newkey','rsa:2048','-nodes','-keyout',key,'-out',dir+'/server.csr','-subj','/CN=localhost'],{stdio:'ignore'});
writeFileSync(dir+'/server.ext','subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n',{mode:0o600});
execFileSync('openssl',['x509','-req','-in',dir+'/server.csr','-CA',ca,'-CAkey',dir+'/ca.key','-CAcreateserial','-out',cert,'-extfile',dir+'/server.ext','-days','1'],{stdio:'ignore'});
await start('tls','/usr/bin/socat',[`OPENSSL-LISTEN:3152,bind=127.0.0.1,reuseaddr,fork,cert=${cert},key=${key},verify=0`,'TCP:127.0.0.1:3151'],{PATH:'/usr/bin:/bin'});
const base={PATH:'/usr/local/bin:/usr/bin:/bin',LANG:'C.UTF-8',RUST_LOG:'off'};
await start('runtime',resolve('.runtime-test/database-runtime/tabularis'),['web','--host','127.0.0.1','--port','3151','--auth','proxy','--no-open','--public-url',fixture.runtimeOrigin,'--allowed-origin',fixture.runtimeOrigin,'--web-root',resolve('.runtime-test/database-runtime/web')],{...base,TABULARIS_DATA_DIR:dir,TABULARIS_WEB_PROXY_SECRET:fixture.proxySecret,TMPDIR:dir,XDG_CONFIG_HOME:dir,XDG_DATA_HOME:dir,XDG_CACHE_HOME:dir});
const pin=JSON.parse(readFileSync('vendor/tabularis-web-ui.json','utf8'));
await start('gateway',resolve('target/debug/webdock-database-gateway'),[],{...base,HOST:'127.0.0.1',PORT:'3150',WEBDOCK_AUTH_ORIGIN:'http://localhost:3125',WEBDOCK_DATABASE_GATEWAY_ORIGIN:fixture.gatewayOrigin,WEBDOCK_DATABASE_BROWSER_ORIGINS:'http://127.0.0.1:3121',WEBDOCK_DATABASE_RUNTIME_ORIGINS:fixture.runtimeOrigin,WEBDOCK_DATABASE_GATEWAY_SECRET:fixture.gatewaySecret,WEBDOCK_DATABASE_RUNTIME_CA_FILE:ca,TABULARIS_EXPECTED_COMMIT:pin.sourceCommit});
const runtimeReady=()=>new Promise(resolve=>{https.get(fixture.runtimeOrigin+'/api/v1/session',{ca:readFileSync(ca),headers:{Origin:fixture.runtimeOrigin,'X-Tabularis-User':fixture.userID,'X-Tabularis-Proxy-Secret':fixture.proxySecret}},response=>{response.resume();resolve(response.statusCode===200);}).on('error',()=>resolve(false));});
for(const origin of [fixture.runtimeOrigin,fixture.gatewayOrigin]){let ready=false;for(let i=0;i<100&&!ready;i++){try{ready=origin===fixture.runtimeOrigin?await runtimeReady():(await fetch(origin+'/healthz')).ok;}catch{}if(!ready)await new Promise(r=>setTimeout(r,100));}if(!ready)throw Error('Owned local database process did not start');}
console.log('Owned Rust database gateway and isolated database runtime ready.');
