// Opt-in actual Console lifecycle. Reuses native CLI/MFA/initial install acceptance on owned fixtures.
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { chromium } from 'playwright';
import { fromRepo } from './repo.mjs';
const reportPath=process.env.WEBDOCK_NATIVE_UPDATE_FIXTURE;
assert.ok(reportPath?.startsWith(fromRepo('.runtime-test/native-plugin-updates/').pathname),'An owned update fixture is required');
const readJSON=async path=>JSON.parse(await readFile(path,'utf8'));
const report=await readJSON(reportPath);assert.equal(report.fixture,true);
const {paths}=report,directory=paths.directory,statePath=directory+'/acceptance-state.json',cookiesPath=directory+'/acceptance-cookies.json';
let state;try{state=await readJSON(statePath);}catch(error){if(error.code!=='ENOENT')throw error;state={};}
if(!state.initialAcceptanceComplete){
 const result=spawnSync('bun',['apps/console/tests/plugin-installation-native.mjs'],{env:{...process.env,WEBDOCK_NATIVE_INSTALL_FIXTURE:reportPath},encoding:'utf8'});
 await writeFile(directory+'/initial-browser.log',(result.stdout??'')+(result.stderr??''),{mode:0o600});
 assert.equal(result.status,0,'Initial native acceptance failed; inspect its sanitized log');
 state=await readJSON(statePath);state.initialAcceptanceComplete=true;await writeFile(statePath,JSON.stringify(state),{mode:0o600});
}
const core=await readJSON(paths.coreEnvironment),catalog=await readJSON(paths.catalog),operator=await readJSON(paths.operatorInput);
const origin=core.WEBDOCK_CONSOLE_ORIGIN;assert.equal(origin,'http://127.0.0.1:6121');
const browser=await chromium.launch({headless:true});
const context=await browser.newContext({storageState:await readJSON(cookiesPath),viewport:{width:1280,height:900}});
const page=await context.newPage();page.setDefaultTimeout(15000);
const errors=[];page.on('pageerror',error=>errors.push(error.message));
const secrets=[state.originalPassword,state.replacement,state.totpSecret,operator.password].filter(Boolean);
const save=()=>writeFile(statePath,JSON.stringify(state),{mode:0o600});
const session=async()=> (await (await page.request.get(origin+'/api/console/session')).json()).data;
async function post(path,data){const response=await page.request.post(origin+path,{data,headers:{Origin:origin}});assert.equal(response.status(),200,'Owned native request rejected');return response.json();}
async function exitOwner(){const s=await session();assert.equal(s.user.email,operator.email);if(s.user.preview)await post('/api/console/tenant-preview/exit',{});}
async function actOwner(){const s=await session();if(!s.user.acting){assert.equal(s.user.role,'operator');await page.goto(origin+'/customers/'+state.customerID);await page.getByRole('button',{name:'Act as customer owner',exact:true}).click();await page.waitForURL(origin+'/workspaces/'+state.customerID);}const scope=await post('/api/console/tenant-preview/current',{});assert.equal(scope.customerID,state.customerID);}
const resourceURL=origin+`/workspaces/${state.customerID}/projects/${state.projectID}/resources/postgres/${state.resourceID}`;
async function resource(){
 await actOwner();await page.goto(resourceURL);await page.getByText('Ready',{exact:true}).waitFor({timeout:60000});
 const view=await post('/api/console/plugins/invoke',{pluginID:'postgres',projectID:state.projectID,action:'databases.view',input:{resource_id:state.resourceID},idempotencyKey:crypto.randomUUID()});
 const reply=page.waitForResponse(response=>new URL(response.url()).pathname==='/api/console/plugins/invoke'&&response.request().postDataJSON()?.action==='databases.credentials');
 await page.getByRole('button',{name:'Show password',exact:true}).click();const secret=await(await reply).json();secrets.push(secret.value,secret.uri);
 await page.getByRole('button',{name:'Hide',exact:true}).click();return{view,password:secret.value};
}
const {Client}=await import('pg');const ca=await readFile(process.env.WEBDOCK_NATIVE_DATABASE_CA??fromRepo('.dev-local/ca/ca.crt'));
async function readSentinel(value){const sql=new Client({host:'127.0.0.1',port:value.view.port,database:value.view.database,user:value.view.user,password:value.password,ssl:{ca,servername:value.view.host,rejectUnauthorized:true},connectionTimeoutMillis:10000,query_timeout:10000});await sql.connect();try{assert.equal((await sql.query('SELECT value FROM native_installer_acceptance')).rows[0].value,42);}finally{await sql.end();}}
function kube(args){const result=spawnSync('kubectl',['--kubeconfig',paths.kubeconfig,'--context','webdock-dev',...args],{encoding:'utf8'});assert.equal(result.status,0,'Owned Kubernetes observation failed');return result.stdout;}
async function observeGenerations(current,version){
 const namespaces=JSON.parse(kube(['get','namespaces','-l',`webdock.dev/platform=${catalog.platformID},webdock.dev/plugin=postgres`,'-o','json'])).items;
 const ledger=await readJSON(report.ledger);ledger.namespaces??=[];
 for(const namespace of namespaces){const name=namespace.metadata.name,generation=namespace.metadata.labels['webdock.dev/runtime-generation'];
  if(!ledger.namespaces.some(row=>row.uid===namespace.metadata.uid))ledger.namespaces.push({name,uid:namespace.metadata.uid});
  const deployment=JSON.parse(kube(['-n',name,'get','deployment','runtime','-o','json']));
  const pods=JSON.parse(kube(['-n',name,'get','pods','-o','json'])).items;
  if(generation===current){assert.equal(deployment.spec.replicas,1);assert.equal(deployment.status.availableReplicas,1);const release=catalog.releases.find(row=>row.version===version);assert.equal(deployment.spec.template.spec.containers[0].image,JSON.parse(release.manifestRaw).image);state.runtimeNamespaces??={};state.runtimeNamespaces[current]=name;}
  else{assert.equal(deployment.spec.replicas,0,'superseded runtime is scaled down');assert.equal(pods.length,0,'superseded runtime has no terminating or unready Pods');}
 }
 const customer=JSON.parse(kube(['get','namespace','wd-postgres-'+state.projectID,'-o','json']));
 if(!ledger.namespaces.some(row=>row.uid===customer.metadata.uid))ledger.namespaces.push({name:customer.metadata.name,uid:customer.metadata.uid});
 await writeFile(report.ledger,JSON.stringify(ledger,null,2),{mode:0o600});
 assert.ok(state.runtimeNamespaces?.[current],'promoted runtime generation namespace observed');await save();
}
async function waitOperation(id){
 const deadline=Date.now()+240000;let snapshot;
 do{const response=await page.request.get(origin+'/api/console/plugins/operations/'+id);assert.equal(response.status(),200);snapshot=await response.json();if(['succeeded','failed','blocked'].includes(snapshot.status))break;await new Promise(resolve=>setTimeout(resolve,1000));}while(Date.now()<deadline);
 assert.equal(snapshot.status,'succeeded','Lifecycle stopped: '+(snapshot.condition??snapshot.status));
 await page.goto(origin+'/extensions/operations/'+id);await page.getByText('succeeded',{exact:true}).waitFor();
}
async function change(kind,version,generation){
 await exitOwner();await page.goto(origin+'/extensions/postgres/releases');await page.getByRole('heading',{name:'postgres versions',exact:true}).waitFor();
 const key=kind+'OperationID';
 if(!state[key]){
  const installed=await(await page.request.get(origin+'/api/console/plugins/installed')).json();
  if(installed.find(row=>row.id==='postgres').state!=='disabled'){
   await page.getByRole('checkbox',{name:'Disable this extension before changing its version.'}).check();await page.getByRole('button',{name:'Disable for version change',exact:true}).click();
  }
  const review=page.getByRole('button',{name:`Review ${kind==='update'?'update':'rollback'} to ${version}`,exact:true});
  const deadline=Date.now()+120000;while(await review.isDisabled()){assert.ok(Date.now()<deadline,'Disable/drain did not finish');await new Promise(resolve=>setTimeout(resolve,1000));}
  await review.click();await page.getByRole('checkbox',{name:'I reviewed this release, its permissions and schema compatibility.'}).check();
  const reply=page.waitForResponse(response=>new URL(response.url()).pathname==='/api/console/plugins/postgres/'+kind&&response.request().method()==='POST');
  await page.getByRole('button',{name:kind==='update'?'Update extension':'Roll back extension',exact:true}).click();const response=await reply;assert.equal(response.status(),200,'Native lifecycle admission');state[key]=(await response.json()).id;await save();
 }
 const before=await(await page.request.get(origin+'/api/console/plugins/operations/'+state[key])).json();
 if(['failed','blocked'].includes(before.status)){
  assert.equal(process.env.WEBDOCK_NATIVE_UPDATE_RETRY,'true','Lifecycle requires an explicit retained retry: '+before.condition);
  await page.goto(origin+'/extensions/operations/'+state[key]);
  const reply=page.waitForResponse(response=>new URL(response.url()).pathname.endsWith('/retry')&&response.request().method()==='POST');
  await page.getByRole('button',{name:'Retry '+kind,exact:true}).click();const response=await reply;assert.equal(response.status(),200);state[key]=(await response.json()).id;await save();
 }
 await waitOperation(state[key]);
 const history=await(await page.request.get(origin+'/api/console/plugins/postgres/releases')).json();assert.equal(history.currentGeneration,generation);assert.equal(history.candidateGeneration,null);
 await observeGenerations(generation,version);
 const value=await resource();assert.equal(value.password,state.originalPassword,'customer password and sealing key retained');assert.deepEqual({host:value.view.host,port:value.view.port,user:value.view.user,database:value.view.database},state.databaseIdentity,'retained customer resource');await readSentinel(value);
 console.log(`Native ${kind} passed: release ${version}, fresh runtime generation ${generation}, old Pods absent, retained credentials/SQL.`);
}
try{
 if(!state.originalPassword){const baseline=await resource();state.originalPassword=baseline.password;state.databaseIdentity={host:baseline.view.host,port:baseline.view.port,user:baseline.view.user,database:baseline.view.database};secrets.push(baseline.password);await readSentinel(baseline);await save();}
 if(!state.updateVerified){
  if(!state.runtimeNamespaces?.['1'])await observeGenerations('1','0.1.0');
  state.phase='update';await save();await change('update','0.2.0','2');state.updateVerified=true;await save();
 }
 if(!state.rollbackVerified){state.phase='rollback';await save();await change('rollback','0.1.0','3');state.rollbackVerified=true;await save();}
 await observeGenerations('3','0.1.0');
 const finalValue=await resource();assert.equal(finalValue.password,state.originalPassword);await readSentinel(finalValue);
 assert.notEqual(state.runtimeNamespaces['1'],state.runtimeNamespaces['2']);assert.notEqual(state.runtimeNamespaces['2'],state.runtimeNamespaces['3']);
 assert.deepEqual(errors,[]);state.phase='updates-complete';await save();
 console.log('Native compatible code-only v1 -> v2 -> v1 lifecycle complete; physical SQL schema migration is not claimed.');
}catch(error){let message=String(error.message).split('\n')[0];for(const secret of secrets)if(secret)message=message.replaceAll(secret,'[redacted]');console.error('Native version acceptance failed at '+state.phase+': '+message);process.exitCode=1;}
finally{await writeFile(cookiesPath,JSON.stringify(await context.storageState()),{mode:0o600});await context.close();await browser.close();}
