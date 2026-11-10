/** Synthetic restricted-role SQL fixture only; never production or provider I/O. */
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {randomBytes} from 'node:crypto';
import {Pool} from 'pg';
import {withDatabaseScope} from '@spitzli/database-runtime';
import {createDatabases} from '@spitzli/adapter-database-postgres';

const input=process.env.SPITZLI_PUBLIC_HOST_FIXTURE;
test('platform hostname authority survives races and deletion without crossing tenants',{skip:!input,timeout:60000},async()=>{
 const f=JSON.parse(await readFile(input!,'utf8'));
 for(const raw of [f.ownerUrl,...Object.values(f.roles) as string[]]){
  const u=new URL(raw);assert.equal(u.hostname,'127.0.0.1');assert.equal(u.pathname,'/webdock_rls_test');assert.equal(u.search,'');
 }
 const owner=new Pool({connectionString:f.ownerUrl,max:1});
 const pools=createDatabases({environment:'test',businessURL:f.roles.webdock_auth_runtime,authURL:f.roles.webdock_auth_bootstrap,workerURL:f.roles.webdock_worker_runtime,signingSecret:f.contextSecret,max:4});
 const base=BigInt('84'+BigInt('0x'+randomBytes(4).toString('hex')).toString());let next=base+100n;const id=()=>String(next++);
 const [a,b,sa,sb,ca,cb,pa,pb,cluster,aa,ab,ac,ba,bb]=Array.from({length:14},(_,n)=>String(base+BigInt(n)));
 const session=(user:string,sid:string,fn:()=>Promise<any>)=>withDatabaseScope({kind:'session',subject:user,sessionID:sid},fn);
 const asA=(fn:()=>Promise<any>)=>session(a,sa,fn);const asB=(fn:()=>Promise<any>)=>session(b,sb,fn);
 const allocate=(app:string,label:string|null)=>pools.business.query('SELECT webdock_rls.allocate_platform_hostname($1,$2,$3) AS allocation',[id(),app,label]);
 const denied=async(fn:()=>Promise<any>,codes:string[])=>{await assert.rejects(fn,error=>codes.includes((error as {code:string}).code));};
 try{
  await owner.query(`INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword") VALUES($1,'Owner A',$1||'@example.invalid','user',true,false),($2,'Owner B',$2||'@example.invalid','user',true,false)`,[a,b]);
  await owner.query(`INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval '1 hour',now()),($3,$3,$4,now()+interval '1 hour',now())`,[sa,a,sb,b]);
  for(const [customer,name] of [[ca,'Address fixture A'],[cb,'Address fixture B']]){
   const client=await owner.connect();
   try{await client.query('BEGIN');await client.query("SELECT set_config('webdock.organization_allocation',$1,true)",[JSON.stringify({customer_id:customer,organization_id:id()})]);await client.query("INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,$2,'active')",[customer,name]);await client.query('COMMIT');}catch(error){await client.query('ROLLBACK');throw error;}finally{client.release()}
  }
  for(const [user,customer] of [[a,ca],[b,cb]])await owner.query(`INSERT INTO webdock_auth.member(id,"userId","organizationId",role,"createdAt") SELECT $1,$2,organization_id,'owner',now() FROM webdock_auth.tenant_customer WHERE customer_id=$3`,[id(),user,customer]);
  await owner.query("INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES($1,'Project A',$2),($3,'Project B',$4)",[pa,ca,pb,cb]);
  await owner.query("INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,runtime_kind,runtime_installation,verified,ownership) VALUES($1,'Public address fixture','k3s','EU','owned local fixture','kubernetes','fixture',true,'platform')",[cluster]);
  await owner.query("INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,cluster_id,mode,own_images,namespace) VALUES($1,$2,'k3s',$3,'selfservice',true,$6),($4,$5,'k3s',$3,'selfservice',true,$7)",[pa,ca,cluster,pb,cb,'wd-'+pa,'wd-'+pb]);
  await owner.query("INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_sequence,last_seen,observation) VALUES($1,$2,1,1,now(),'{\"capabilities\":{\"publicRoutingVersion\":1}}')",[cluster,randomBytes(32).toString('hex')]);
  for(const [app,project,name]of [[aa,pa,'Lunares'],[ab,pa,'Collision'],[ac,pa,'Next A'],[ba,pb,'Other'],[bb,pb,'Next B']])await owner.query("INSERT INTO webdock_auth.hosting_app(id,project_id,name,spec) VALUES($1,$2,$3,'{}')",[app,project,name]);
  await owner.query("INSERT INTO webdock_auth.public_hosting_domain(domain,cluster_id,enabled,tenant_daily_limit,global_daily_limit,trusted_customers,configured_by) VALUES('spitzli.site',$1,true,3,4,$2::text[],'fixture')",[cluster,[ca,cb]]);
  const first=(await asA(()=>allocate(aa,'lunares'))).rows[0].allocation;assert.equal(first.hostname,'lunares.spitzli.site');
  assert.equal((await asA(()=>pools.business.query("UPDATE webdock_auth.public_hosting_domain SET global_daily_limit=1000 WHERE domain='spitzli.site'"))).rowCount,0);
  assert.equal((await owner.query("SELECT global_daily_limit FROM webdock_auth.public_hosting_domain WHERE domain='spitzli.site'")).rows[0].global_daily_limit,4);
  await owner.query("UPDATE webdock_auth.public_hosting_domain SET enabled=false WHERE domain='spitzli.site'");
  assert.equal((await asA(()=>allocate(aa,'lunares'))).rows[0].allocation.id,first.id,'closing admission retains an existing assignment');
  await denied(()=>asA(()=>allocate(ab,'new-disabled')),['42501']);
  await owner.query("UPDATE webdock_auth.public_hosting_domain SET enabled=true WHERE domain='spitzli.site'");

  const repeated=(await asA(()=>allocate(aa,'lunares'))).rows[0].allocation;assert.equal(repeated.id,first.id);
  await denied(()=>asA(()=>allocate(aa,'different')),['23505']);
  await denied(()=>asB(()=>allocate(aa,'steal')),['42501']);
  assert.equal((await asB(()=>pools.business.query('SELECT id FROM webdock_auth.platform_hostname'))).rowCount,0);
  assert.equal((await pools.business.query('SELECT id FROM webdock_auth.platform_hostname')).rowCount,0);
  for(const label of ['api','auth','admin','*','upper.CASE','p-123'])await denied(()=>asA(()=>allocate(ab,label)),['22023']);
  const raced=await Promise.allSettled([asA(()=>allocate(ab,'collision')),asB(()=>allocate(ba,'collision'))]);assert.equal(raced.filter(x=>x.status==='fulfilled').length,1);assert.equal(raced.filter(x=>x.status==='rejected'&&(x.reason as {code:string}).code==='23505').length,1);
  assert.equal((await owner.query("SELECT count(*)::int AS count FROM webdock_auth.platform_hostname WHERE hostname='collision.spitzli.site'")).rows[0].count,1);
  await denied(()=>asA(()=>pools.business.query("UPDATE webdock_auth.platform_hostname SET hostname='hijack.spitzli.site' WHERE id=$1",[first.id])),['42501']);
  await denied(()=>asA(()=>pools.business.query('DELETE FROM webdock_auth.platform_hostname WHERE id=$1',[first.id])),['42501']);
  await asA(()=>pools.business.query("UPDATE webdock_auth.hosting_app SET status='deleted' WHERE id=$1",[aa]));
  const deletedDraft=(await owner.query('SELECT state,revision FROM webdock_auth.platform_hostname WHERE id=$1',[first.id])).rows[0];assert.equal(deletedDraft.state,'retired');assert.equal(Number(deletedDraft.revision),2);
  const parent='d'+cluster+'.invalid';const suffix='apps.'+parent;const domainID=id();
  await owner.query("INSERT INTO webdock_auth.customer_domain(id,customer_id,domain,token,verified_at,created_by) VALUES($1,$2,$3,$4,now(),$5)",[domainID,ca,parent,'a'.repeat(32),a]);
  await denied(()=>owner.query("INSERT INTO webdock_auth.public_hosting_domain(domain,enabled,tenant_daily_limit,global_daily_limit,configured_by) VALUES($1,false,1,1,'fixture')",[suffix]),['42501']);
  await owner.query('DELETE FROM webdock_auth.customer_domain WHERE id=$1',[domainID]);
  await owner.query("INSERT INTO webdock_auth.public_hosting_domain(domain,enabled,tenant_daily_limit,global_daily_limit,configured_by) VALUES($1,false,1,1,'fixture')",[suffix]);
  assert.equal((await asA(()=>pools.business.query('SELECT webdock_rls.platform_host_reserved($1) AS reserved',[parent]))).rows[0].reserved,true);
  await denied(()=>asA(()=>pools.business.query("INSERT INTO webdock_auth.customer_domain(id,customer_id,domain,token,verified_at,created_by) VALUES($1,$2,$3,$4,now(),$5)",[id(),ca,parent,'b'.repeat(32),a])),['42501']);
  const plugin='domain-fixture-'+cluster;
  await owner.query("INSERT INTO webdock_auth.plugin_installation(id,kind,desired_state,state,version) VALUES($1,'external','enabled','enabled','1.0.0')",[plugin]);
  await denied(()=>owner.query("INSERT INTO webdock_auth.plugin_workload(plugin_id,unit,project_id,cluster_id,namespace,objects,hosts,customer_id) VALUES($1,'probe',$2,$3,$4,'[]',$5,$6)",[plugin,pa,cluster,'wd-'+pa,['victim.spitzli.site'],ca]),['42501']);
  await owner.query('DELETE FROM webdock_auth.plugin_installation WHERE id=$1',[plugin]);
  await owner.query('DELETE FROM webdock_auth.public_hosting_domain WHERE domain=$1',[suffix]);
  for(let attempt=0;attempt<3;attempt++) {
   const parent='race-'+attempt+'-'+cluster+'.invalid',suffix='apps.'+parent,domainID=id();
   const outcomes=await Promise.allSettled([
    owner.query("INSERT INTO webdock_auth.public_hosting_domain(domain,enabled,tenant_daily_limit,global_daily_limit,configured_by) VALUES($1,false,1,1,'fixture')",[suffix]),
    asA(()=>pools.business.query("INSERT INTO webdock_auth.customer_domain(id,customer_id,domain,token,verified_at,created_by) VALUES($1,$2,$3,$4,now(),$5)",[domainID,ca,parent,'a'.repeat(32),a]))
   ]);
   assert.equal(outcomes.filter(x=>x.status==='fulfilled').length,1,'policy and overlapping custom-domain verification cannot both win');
   assert.equal(outcomes.filter(x=>x.status==='rejected'&&(x.reason as {code:string}).code==='42501').length,1);
   await owner.query('DELETE FROM webdock_auth.customer_domain WHERE id=$1',[domainID]);await owner.query('DELETE FROM webdock_auth.public_hosting_domain WHERE domain=$1',[suffix]);
  }


  const reserved=(await asA(()=>pools.business.query("SELECT webdock_rls.platform_host_reserved('x.spitzli.site') AS child,webdock_rls.platform_host_reserved('spitzli.site') AS apex,webdock_rls.platform_host_reserved('spitzli.site.evil') AS lookalike"))).rows[0];assert.deepEqual(reserved,{child:true,apex:true,lookalike:false});
  await owner.query('DELETE FROM webdock_auth.hosting_app WHERE id=$1',[aa]);
  const tomb=(await owner.query('SELECT state,hostname,app_id FROM webdock_auth.platform_hostname WHERE id=$1',[first.id])).rows[0];assert.equal(tomb.state,'retired');assert.equal(tomb.hostname,'lunares.spitzli.site');assert.equal(tomb.app_id,null);
  await denied(()=>asA(()=>allocate(ac,'lunares')),['23505']);
  await owner.query("UPDATE webdock_auth.public_hosting_domain SET tenant_daily_limit=1,global_daily_limit=4 WHERE domain='spitzli.site'");
  await denied(()=>asA(()=>allocate(ac,'budget-a')),['54000']);
  await owner.query("UPDATE webdock_auth.public_hosting_domain SET tenant_daily_limit=4,global_daily_limit=4 WHERE domain='spitzli.site'");
  await asA(()=>allocate(ac,'next-a'));await asB(()=>allocate(bb,'next-b'));
  const extra=id();await owner.query("INSERT INTO webdock_auth.hosting_app(id,project_id,name,spec) VALUES($1,$2,'Beyond budget','{}')",[extra,pa]);
  await denied(()=>asA(()=>allocate(extra,'global-budget')),['54000']);
  await owner.query('DELETE FROM webdock_auth.hosting_app WHERE project_id=$1',[pa]);
  await owner.query('DELETE FROM webdock_auth.hosting_project WHERE project_id=$1',[pa]);
  await owner.query('DELETE FROM webdock_admin.projects WHERE id=$1',[pa]);
  assert.equal((await owner.query("SELECT count(*)::int AS count FROM webdock_auth.platform_hostname WHERE allocated_customer_id=$1 AND state='retired'",[ca])).rows[0].count>=2,true);
  assert.equal((await owner.query('SELECT count(*)::int AS count FROM webdock_auth.platform_hostname')).rows[0].count,4);
 }finally{
  if((await owner.query("SELECT to_regclass('webdock_auth.platform_hostname') AS table_name")).rows[0].table_name)await owner.query('DELETE FROM webdock_auth.platform_hostname WHERE allocated_customer_id=ANY($1)',[[ca,cb]]);
  if((await owner.query("SELECT to_regclass('webdock_auth.public_hosting_domain') AS table_name")).rows[0].table_name)await owner.query('DELETE FROM webdock_auth.public_hosting_domain WHERE cluster_id=$1',[cluster]);
  await pools.close();await owner.end();
 }
});
