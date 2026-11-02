/** Opt-in setup hook for the owned disposable core browser fixture. */
import {readFile,writeFile,open} from 'node:fs/promises';
import {createHash,generateKeyPairSync,randomBytes} from 'node:crypto';
import {spawn,spawnSync} from 'node:child_process';
import {join} from 'node:path';
import {CompactSign,exportJWK} from 'jose';
import {activatePinnedPlugin} from '../plugins/activate-pinned';
export async function preparePluginBrowser({dir,root,owner,secrets,env,offlineURL,target,children}:any){
const id='synthetic-acceptance-notes',project='980000000000000001',customer='980000000000000002';
await owner.query("INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,'Synthetic acceptance customer','active')",[customer]);
await owner.query("INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1,'Synthetic acceptance project',$2,'active')",[project,customer]);
const org=(await owner.query('SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1',[customer])).rows[0].organization_id;
await owner.query('INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',[org,secrets.operatorID]);
// Register only this owned Console as a local MCP resource server; bearer clients still require real consent/PKCE.
await owner.query(`INSERT INTO webdock_auth."oauthResource"(identifier,name,"allowedScopes",disabled,"createdAt","updatedAt") VALUES($1,'Synthetic local MCP resource',$2::jsonb,false,now(),now()) ON CONFLICT(identifier) DO NOTHING`,[env.WEBDOCK_MCP_RESOURCE,JSON.stringify(['webdock:read','webdock:write','offline_access'])]);
await owner.query('INSERT INTO webdock_auth."oauthClientResource"("clientId","resourceId","createdAt") VALUES($1,$2,now()) ON CONFLICT DO NOTHING',[secrets.clientID,env.WEBDOCK_MCP_RESOURCE]);
const cert=await readFile(join(dir,'tls/cert.pem'),'utf8'),key=await readFile(join(dir,'tls/key.pem'),'utf8');
const pair=generateKeyPairSync('ed25519'),credential=randomBytes(32).toString('hex'),privateKey=pair.privateKey.export({type:'pkcs8',format:'pem'}).toString(),publicKey=pair.publicKey.export({type:'spki',format:'pem'}).toString();
const runtime={privateKey,hostOrigin:'https://localhost:3444',runtimes:{[id]:{generation:1,namespace:'plugin-browser',service:'fixture',addresses:['127.0.0.1'],credential,ca:cert,projects:{[project]:{}},uiOrigin:'http://localhost:3446'}}};
const runtimePath=join(dir,'runtime.json');await writeFile(runtimePath,JSON.stringify(runtime),{mode:0o600});env.WEBDOCK_PLUGIN_RUNTIME_FILE=runtimePath;
const manifest=JSON.parse(await readFile(join(root,'tools/plugins/fixtures/project-notes.json'),'utf8'));
Object.assign(manifest,{name:id,description:'Synthetic local acceptance extension; never published',capabilities:['plugin.data.read','plugin.data.write'],jobs:[],navigation:[{id:'notes',label:'Synthetic acceptance notes',path:'/ui/panel'}],api:[{method:'GET',path:'/notes',action:'notes.read'},{method:'POST',path:'/notes',action:'notes.write'}],mcp:[{name:'read',description:'Read synthetic project note',action:'notes.read'},{name:'write',description:'Write synthetic project note',action:'notes.write'}]});
manifest.actions[1].capabilities=['plugin.data.read','plugin.data.write'];manifest.actions[1].input={type:'object',properties:{note:{type:'string',maxLength:2000}},required:['note'],additionalProperties:false};
const raw=JSON.stringify(manifest),digest=createHash('sha256').update(raw).digest('hex'),registryURL='https://synthetic-registry.invalid',kid='synthetic-local-key';
const payload={v:1,registry:registryURL,plugin_slug:id,release_version:'1.0.0',kid,manifest_sha256:digest,assets:[{name:id+'-universal.json',sha256:digest,size:Buffer.byteLength(raw)}]};
const jws=await new CompactSign(Buffer.from(JSON.stringify(payload))).setProtectedHeader({alg:'EdDSA',kid}).sign(pair.privateKey);
const input={release:{registryURL,id,version:'1.0.0',kid,key:await exportJWK(pair.publicKey),jws,manifestRaw:raw,descriptor:raw},projectID:project,customerID:customer,runtime:{namespace:'plugin-browser',service:'fixture',credentialHash:createHash('sha256').update(credential).digest('hex'),clusterID:'owned-local-browser',generation:1},approvedCapabilities:manifest.capabilities,configuration:{}};
const inputPath=join(dir,'plugin-install.json');await writeFile(inputPath,JSON.stringify(input),{mode:0o600});
const offlineEnv={...env,DATABASE_URL_UNPOOLED:offlineURL,DATABASE_CA_CERT:cert};
const installed=spawnSync(process.execPath,['--no-env-file','tools/plugins/install-pinned.ts',inputPath,target],{cwd:root,env:offlineEnv,encoding:'utf8'});if(installed.status!==0)throw Error('Pinned installation failed: '+installed.stderr);
const privatePath=join(dir,'plugin-process.json');await writeFile(privatePath,JSON.stringify({id,cert,key,publicKey,credential}),{mode:0o600});
const log=await open(join(dir,'plugin.log'),'a',0o600);const child=spawn(process.execPath,['--no-env-file','tools/tests/plugin-browser-runtime.ts',privatePath],{cwd:root,env:{PATH:process.env.PATH,HOME:process.env.HOME},stdio:['ignore',log.fd,log.fd]});children.push(child);await log.close();
for(let n=0;;n++){try{const r=await fetch('https://localhost:3443/healthz',{headers:{authorization:'Bearer '+credential},tls:{ca:cert}} as any);if(r.ok)break;}catch{}if(n>40)throw Error('Runtime startup failed');await new Promise(r=>setTimeout(r,100));}
const activated=await activatePinnedPlugin(offlineEnv,id);
await writeFile(join(dir,'plugin-manifest.json'),JSON.stringify({synthetic:true,project,customer,id,installed:JSON.parse(installed.stdout),activated,panel:'http://localhost:3460/plugins/'+project,uiOrigin:'http://localhost:3446',uiTransport:'explicit-development-loopback-http',actionsTransport:'private-verified-TLS',callbackTransport:'private-verified-TLS'}),{mode:0o600});
}
