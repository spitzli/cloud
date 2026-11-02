// HTTP fixtures only. Real generation/Pod/SQL acceptance is a separate opt-in native run.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
const release = (version) => ({ catalogID:'postgres',pluginID:'postgres',version,manifestSha256:(version==='0.1.0'?'a':'b').repeat(64),image:`registry.example.test/postgres@sha256:${'c'.repeat(64)}`,registryURL:'https://registry.example.test',signerKeyID:'owned-key',capabilities:['plugin.workloads'],supportedFeatures:['create-dedicated'],targets:[{id:'eu',label:'EU',reviewSha256:'d'.repeat(64)}],configurationSchema:{type:'object',properties:{},additionalProperties:false},schemaVersion:1,readableSchema:{min:1,max:1} });
const browser=await chromium.launch({headless:true});
async function open(mode='operator',highWater=1){
 const context=await browser.newContext({viewport:{width:1280,height:900}});const page=await context.newPage();page.setDefaultTimeout(7000);
 const state={disabled:false,disableComplete:false,updateComplete:false,rollbackComplete:false,lost:true,writes:[],globalReads:0,failOperationReads:false,operationReads:0};
 const path=(p)=>(url)=>url.pathname===p;const json=(body)=>(route)=>route.fulfill({json:body});
 await page.route(path('/console-config.json'),json({apiOrigin:previewOrigin,authOrigin:previewOrigin,enabledFeatures:[]}));
 await page.route(path('/v1/auth/session'),json({data:{user:{id:'2',name:'Fixture',email:'fixture@example.test',role:mode==='operator'?'operator':'user',preview:mode==='acting',acting:mode==='acting',emailVerified:true},session:{id:'owned-browser',userId:'2',expiresAt:'2099-01-01T00:00:00Z'}}}));
 await page.route(path('/api/console/tenants'),json(mode==='operator'?[]:[{id:'101',name:'Fixture',role:'owner',status:'active'}]));
 await page.route(path('/api/console/plugins'),json([]));
 await page.route(path('/api/console/tenant-preview/current'),json({customerID:'101',customerName:'Fixture',status:'active',mode:'owner',readOnly:false,expiresAt:'2099-01-01T00:00:00Z'}));
 const current=()=>state.rollbackComplete?'3':state.updateComplete?'2':'1';
 const installed=()=>({id:'postgres',kind:'external',revision:state.rollbackComplete?'4':state.updateComplete?'3':state.disabled?'2':'1',desiredState:state.disabled?'disabled':'enabled',state:state.disabled?'disabled':'enabled',version:state.updateComplete&&!state.rollbackComplete?'0.2.0':'0.1.0',imageDigest:null,runtime:'ready',blockers:[],dependencies:[],managed:true,operationID:state.rollbackComplete?'701':state.updateComplete?'601':state.disabled?'501':'101'});
 await page.route(path('/api/console/plugins/installed'),route=>{
  if(route.request().method()==='POST'){const body=route.request().postDataJSON();state.writes.push({kind:'state',body});return route.fulfill({json:{id:'501',status:'pending'}});}
  return route.fulfill({json:[installed()]});
 });
 await page.route(path('/api/console/plugins/catalog'),route=>{state.globalReads++;return route.fulfill({json:[release('0.1.0'),release('0.2.0')]});});
 await page.route(path('/api/console/plugins/postgres/releases'),route=>{
  state.globalReads++;
  const row=(generation,version)=>({generation,version,imageDigest:release(version).image,manifestSha256:release(version).manifestSha256,status:generation===current()?'ready':'stopped',operationID:'101',schemaVersion:1,readableSchema:{min:1,max:1},rollbackEligible:highWater===1,rollbackBlocker:highWater===1?null:'schema_incompatible'});
  return route.fulfill({json:{pluginID:'postgres',currentGeneration:current(),candidateGeneration:null,previousGeneration:state.updateComplete?'1':null,schemaHighWater:highWater,targetID:'eu',releases:[...(state.updateComplete?[row('2','0.2.0')]:[]),row('1','0.1.0')]}});
 });
 await page.route((url)=>/\/api\/console\/plugins\/operations\/\d+$/.test(url.pathname),route=>{
  state.operationReads++;if(state.failOperationReads)return route.fulfill({status:503,json:{error:{code:'unavailable',message:'Operation status temporarily unavailable'}}});
  const id=new URL(route.request().url()).pathname.split('/').at(-1);let done=id==='101',kind='install',phase='complete';
  if(id==='501'){kind='state';done=state.disableComplete;phase=done?'complete':'draining';if(done)state.disabled=true;}
  if(id==='601'){kind='update';done=state.updateComplete;phase=done?'complete':'suspending_runtime';}
  if(id==='701'){kind='rollback';done=state.rollbackComplete;phase=done?'complete':'suspending_runtime';}
  return route.fulfill({json:{id,pluginID:'postgres',kind,status:done?'succeeded':'running',phase,generation:id==='701'?'3':'2',cursor:done?'2':'1',condition:null}});
 });
 for(const kind of ['update','rollback'])await page.route(path('/api/console/plugins/postgres/'+kind),route=>{
  state.writes.push({kind,body:route.request().postDataJSON()});if(kind==='update'&&state.lost){state.lost=false;return route.abort('failed');}
  return route.fulfill({json:{id:kind==='update'?'601':'701',status:'pending'}});
 });
 return{context,page,state};
}
try{
 for(const retryButton of ['Refresh','Try again']){
  const recovery=await open();const {page,state}=recovery;
  await page.goto(previewOrigin+'/extensions/postgres/releases');
  await page.getByRole('heading',{name:'postgres versions',exact:true}).waitFor();
  await page.getByRole('checkbox',{name:'Disable this extension before changing its version.'}).check();
  await page.getByRole('button',{name:'Disable for version change',exact:true}).click();
  await page.getByText('Waiting for accepted work',{exact:true}).waitFor();state.failOperationReads=true;
  await page.getByRole('alert').filter({hasText:'Unable to load'}).waitFor();
  assert.equal(await page.getByRole('button',{name:'Review update to 0.2.0',exact:true}).isDisabled(),true);
  await page.getByRole('button',{name:'Try again',exact:true}).waitFor();
  const failedReads=state.operationReads;state.failOperationReads=false;state.disableComplete=true;
  await page.getByRole('button',{name:retryButton,exact:true}).click();
  await page.waitForFunction(()=>!Array.from(document.querySelectorAll('button')).find(b=>b.textContent==='Review update to 0.2.0')?.disabled);
  assert.ok(state.operationReads>failedReads,'manual retry fetches the failed operation');
  assert.equal(await page.getByRole('alert').count(),0);
  await recovery.context.close();
 }
 const {context,page,state}=await open();
 await page.goto(previewOrigin+'/extensions/postgres/releases');
 await page.getByRole('heading',{name:'postgres versions',exact:true}).waitFor();
 assert.equal(await page.getByRole('button',{name:'Review update to 0.2.0',exact:true}).isDisabled(),true);
 await page.getByRole('checkbox',{name:'Disable this extension before changing its version.'}).check();
 await page.getByRole('button',{name:'Disable for version change',exact:true}).click();
 await page.getByText('Waiting for accepted work', {exact:true}).waitFor();state.disableComplete=true;
 const review=page.getByRole('button',{name:'Review update to 0.2.0',exact:true});await review.waitFor();
 await page.waitForFunction(()=>!Array.from(document.querySelectorAll('button')).find(b=>b.textContent==='Review update to 0.2.0')?.disabled);
 await review.click();
 await page.getByRole('heading',{name:'Update postgres to 0.2.0',exact:true}).waitFor();
 await page.getByRole('checkbox',{name:'I reviewed this release, its permissions and schema compatibility.'}).check();
 await page.getByRole('button',{name:'Update extension',exact:true}).click();
 await page.getByRole('button',{name:'Retry same request',exact:true}).click();
 await page.waitForURL('**/extensions/operations/601');await page.getByText('Stopping previous runtime',{exact:true}).waitFor();
 const writes=state.writes.filter(x=>x.kind==='update');assert.equal(writes.length,2);assert.deepEqual(writes[0].body,writes[1].body);assert.equal(writes[0].body.expectedRevision,'2');
 state.updateComplete=true;await page.getByText('succeeded',{exact:true}).waitFor();
 await page.getByRole('link',{name:'Version history',exact:true}).click();
 await page.getByRole('button',{name:'Review rollback to 0.1.0',exact:true}).click();
 await page.getByRole('checkbox',{name:'I reviewed this release, its permissions and schema compatibility.'}).check();
 await page.getByRole('button',{name:'Roll back extension',exact:true}).click();await page.waitForURL('**/extensions/operations/701');
 assert.equal(state.writes.find(x=>x.kind==='rollback').body.releaseGeneration,'1');state.rollbackComplete=true;
 await page.getByText('succeeded',{exact:true}).waitFor();
 for(const width of[375,1280]){await page.setViewportSize({width,height:900});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);}
 await context.close();
 const blocked=await open('operator',2);blocked.state.disabled=true;blocked.state.updateComplete=true;
 await blocked.page.goto(previewOrigin+'/extensions/postgres/releases');
 await blocked.page.getByRole('heading',{name:'postgres versions',exact:true}).waitFor();
 assert.equal(await blocked.page.getByRole('button',{name:'Review rollback to 0.1.0',exact:true}).isDisabled(),true);await blocked.context.close();
 for(const mode of['customer','acting']){const f=await open(mode);await f.page.goto(previewOrigin+'/extensions/postgres/releases');await f.page.getByRole('heading',{name:'Operator access required',exact:true}).waitFor();assert.equal(f.state.globalReads,0);assert.equal(f.state.writes.length,0);await f.context.close();}
 console.log('Plugin versions UI passed: disable/drain, reviewed update, uncertain replay, rollback, incompatible reader, operation polling recovery and customer/acting denial. HTTP fixtures only.');
}finally{await browser.close();await server.close();}
