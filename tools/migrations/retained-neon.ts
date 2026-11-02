/** Offline preservation cutover only; never imported by a public/runtime handler. */
import {createHash} from 'node:crypto';
import type {PoolClient} from 'pg';
import {z} from 'zod';
const id=z.string().regex(/^[1-9][0-9]{0,18}$/);
const email=z.string().trim().toLowerCase().pipe(z.email());
const targetSchema=z.object({canonicalId:id,email,role:z.literal('user'),issuer:z.literal('https://clerk.webdock.dev'),externalSubject:z.string().regex(/^user_[a-zA-Z0-9]+$/),providerOrigin:z.url().refine(value=>{const u=new URL(value);return u.protocol==='https:'&&!u.username&&!u.password&&!u.port&&!u.search&&!u.hash&&u.hostname.includes('.neonauth.')&&u.hostname.endsWith('.neon.tech')&&u.pathname==='/neondb/auth';}),providerSubject:z.uuid(),snapshotSha256:z.string().regex(/^[a-f0-9]{64}$/)}).strict();
const providerSchema=z.object({issuer:z.string(),externalSubject:z.string(),primaryEmail:email,emailVerified:z.literal(true),banned:z.literal(false),locked:z.literal(false),mfaEnabled:z.literal(false),passkeys:z.literal(0),checkedAt:z.number().finite()}).strict();
export type RetainedNeonTarget=z.infer<typeof targetSchema>;
export function validateRetainedNeonEvidence(target:unknown,provider:unknown,now=Date.now()):RetainedNeonTarget {
 const t=targetSchema.parse(target),p=providerSchema.parse(provider);
 if(p.issuer!==t.issuer||p.externalSubject!==t.externalSubject||p.primaryEmail!==t.email||now-p.checkedAt>300000||p.checkedAt>now+30000)throw Error('Exact fresh source identity and verified primary email required');
 return t;
}
function sort(value:unknown):unknown {
 if(Array.isArray(value))return value.map(sort);
 if(value!==null&&typeof value==='object')return Object.fromEntries(Object.entries(value).sort(([a],[b])=>a.localeCompare(b)).map(([k,v])=>[k,sort(v)]));
 return value;
}
export const snapshotDigest=(value:unknown)=>createHash('sha256').update(JSON.stringify(sort(JSON.parse(JSON.stringify(value))))).digest('hex');
async function values(db:PoolClient,sql:string,subject:string):Promise<Record<string,any>[]> {
 return(await db.query(sql,[subject])).rows.map(r=>r.value);
}
export async function inspectRetainedNeonUser(db:PoolClient,subject:string){
 id.parse(subject);
 const users=await values(db,'SELECT row_to_json(u) AS value FROM webdock_auth."user" u WHERE id=$1',subject);
 if(users.length!==1)throw Error('Exact retained user missing');
 const state={user:users[0],accounts:await values(db,'SELECT row_to_json(a) AS value FROM webdock_auth.account a WHERE "userId"=$1 ORDER BY id',subject),factors:await values(db,'SELECT row_to_json(f) AS value FROM webdock_auth."twoFactor" f WHERE "userId"=$1 ORDER BY id',subject),passkeys:await values(db,'SELECT row_to_json(p) AS value FROM webdock_auth.passkey p WHERE "userId"=$1 ORDER BY id',subject),memberships:await values(db,'SELECT row_to_json(m) AS value FROM webdock_auth.member m WHERE "userId"=$1 ORDER BY id',subject),grants:await values(db,'SELECT row_to_json(g) AS value FROM webdock_auth.project_grant g WHERE user_id=$1 ORDER BY id',subject),bindings:(await db.query(`SELECT jsonb_build_array(source,issuer,external_subject) AS value FROM (SELECT 'legacy'::text AS source,issuer,clerk_user_id AS external_subject FROM webdock_auth.clerk_identity WHERE user_id=$1 UNION ALL SELECT 'mapping',issuer,external_subject FROM webdock_auth.clerk_identity_mapping WHERE local_subject=$1) b ORDER BY source,issuer,external_subject`,[subject])).rows.map(r=>r.value)};
 return{state,sha256:snapshotDigest(state)};
}
export async function inspectRetainedNeonOrganization(db:PoolClient,subject:string){
 id.parse(subject);
 const state={organization:await values(db,'SELECT row_to_json(o) AS value FROM webdock_auth.organization o WHERE id=$1',subject),memberships:await values(db,'SELECT row_to_json(m) AS value FROM webdock_auth.member m WHERE "organizationId"=$1 ORDER BY id',subject),grants:await values(db,'SELECT row_to_json(g) AS value FROM webdock_auth.project_grant g WHERE organization_id=$1 ORDER BY id',subject),bindings:(await db.query("SELECT jsonb_build_array('organization',issuer,clerk_organization_id) AS value FROM webdock_auth.clerk_organization WHERE organization_id=$1 ORDER BY issuer,clerk_organization_id",[subject])).rows.map(r=>r.value)};
 return{state,sha256:snapshotDigest(state)};
}
export interface RetainedNeonCutover {
 operatorSubject:string;
 evidenceRef:string;
 evidenceSha256:string;
 archiveEnvelope:string;
 users:{target:RetainedNeonTarget;provider:z.infer<typeof providerSchema>;memberships:{organizationId:string;externalOrganization:string;providerRole:string}[]}[];
 organizations:{id:string;snapshotSha256:string;issuer:string;externalOrganization:string}[];
}
export async function activateRetainedNeonUsers(db:PoolClient,input:RetainedNeonCutover){
 id.parse(input.operatorSubject);
 if(input.users.length!==3||new Set(input.users.map(u=>u.target.canonicalId)).size!==3||new Set(input.users.map(u=>u.target.providerSubject)).size!==3)throw Error('Exactly three distinct reviewed retained accounts required');
 if(!input.evidenceRef||!/^([a-f0-9]{64})$/.test(input.evidenceSha256)||input.archiveEnvelope.length<128)throw Error('Encrypted archive and evidence required');
 const archive=JSON.parse(input.archiveEnvelope);
 if(archive.version!==1||archive.algorithm!=='age'||typeof archive.ciphertext!=='string'||!Buffer.from(archive.ciphertext,'base64').subarray(0,32).toString().startsWith('age-encryption.org/v1'))throw Error('Age-encrypted recovery envelope required');
 for(const user of input.users)validateRetainedNeonEvidence(user.target,user.provider);
 await db.query('BEGIN ISOLATION LEVEL SERIALIZABLE');
 try {
  const role=(await db.query('SELECT session_user AS session,current_user AS active,current_database() AS database')).rows[0];
  if(role.session!=='neondb_owner'||role.active!==role.session||role.database!=='neondb')throw Error('Explicit offline Neon owner connection required; runtime roles cannot cut over');
  const policy=(await db.query('SELECT require_managed FROM webdock_auth.neon_auth_policy WHERE singleton FOR UPDATE')).rows[0];
  if(!policy||policy.require_managed)throw Error('Initial all-user policy must remain disabled during reviewed import');
  await db.query('LOCK TABLE webdock_auth."user",webdock_auth.account,webdock_auth."twoFactor",webdock_auth.passkey,webdock_auth.member,webdock_auth.project_grant,webdock_auth.organization,webdock_auth.clerk_identity,webdock_auth.clerk_identity_mapping,webdock_auth.clerk_organization,webdock_auth.local_identity_activation,webdock_auth.local_organization_activation,webdock_auth.neon_identity IN SHARE ROW EXCLUSIVE MODE');
  const operator=(await db.query('SELECT role,"emailVerified",coalesce("twoFactorEnabled",false) AS mfa,coalesce(banned,false) AS banned,webdock_rls.local_identity_authority(id) AS local FROM webdock_auth."user" WHERE id=$1',[input.operatorSubject])).rows[0];
  if(!operator||operator.role!=='operator'||!operator.emailVerified||!operator.mfa||operator.banned||!operator.local)throw Error('Retained secured native operator required');
  const archivedUsers=[];
  for(const item of input.users){
   const {target}=item;const snapshot=await inspectRetainedNeonUser(db,target.canonicalId);const u=snapshot.state.user;
   if(snapshot.sha256!==target.snapshotSha256||u.role!=='user'||!u.emailVerified||u.mustChangePassword||u.banned||u.email.toLowerCase()!==target.email)throw Error('Retained account snapshot or approval changed');
   if(!snapshot.state.bindings.length||snapshot.state.bindings.some(b=>b[1]!==target.issuer||b[2]!==target.externalSubject))throw Error('Exact retained source bindings required');
   if((await db.query('SELECT 1 FROM webdock_auth.local_identity_activation WHERE user_id=$1 UNION ALL SELECT 1 FROM webdock_auth.neon_identity WHERE user_id=$1',[target.canonicalId])).rowCount)throw Error('Account already activated or bound; inspect explicitly before retry');
   const provider=(await db.query('SELECT u.email,coalesce(u.banned,false) AS banned,(SELECT count(*) FROM neon_auth.account a WHERE a."userId"=u.id AND a.password IS NOT NULL) AS credentials,(SELECT count(*) FROM neon_auth.session s WHERE s."userId"=u.id) AS sessions FROM neon_auth."user" u WHERE id=$1',[target.providerSubject])).rows[0];
   if(!provider||provider.email.toLowerCase()!==target.email||provider.banned||provider.credentials!=='0'||provider.sessions!=='0')throw Error('New passwordless provider profile without sessions required');
   if(item.memberships.length!==snapshot.state.memberships.length)throw Error('Every retained membership needs current provider evidence');
   for(const m of snapshot.state.memberships){
    const remote=item.memberships.find(e=>e.organizationId===m.organizationId);const org=input.organizations.find(o=>o.id===m.organizationId);
    if(!remote||!org||remote.externalOrganization!==org.externalOrganization||!['owner','admin','member'].includes(m.role)||!['org:admin','org:member'].includes(remote.providerRole)||(m.role!=='member'&&remote.providerRole!=='org:admin'))throw Error('Current exact organization membership and retained role required');
   }
   archivedUsers.push({item,snapshot});
  }
  const touched=new Set(archivedUsers.flatMap(u=>u.snapshot.state.memberships.map(m=>m.organizationId)));
  if(touched.size!==input.organizations.length||new Set(input.organizations.map(o=>o.id)).size!==input.organizations.length)throw Error('Only exact reviewed member organizations may activate');
  for(const org of input.organizations){
   id.parse(org.id);const snapshot=await inspectRetainedNeonOrganization(db,org.id);
   if(!touched.has(org.id)||snapshot.sha256!==org.snapshotSha256||org.issuer!=='https://clerk.webdock.dev'||!snapshot.state.bindings.length||snapshot.state.bindings.some(b=>b[1]!==org.issuer||b[2]!==org.externalOrganization))throw Error('Exact reviewed organization bindings required');
   if((await db.query('SELECT 1 FROM webdock_auth.local_organization_activation WHERE organization_id=$1',[org.id])).rowCount)throw Error('Organization already activated; inspect explicitly before retry');
   await db.query('UPDATE webdock_auth.clerk_organization SET enabled=false WHERE organization_id=$1',[org.id]);
   await db.query('INSERT INTO webdock_auth.local_organization_activation(organization_id,bindings,approved_state,evidence_ref,evidence_sha256,operator_subject,archive_envelope) VALUES($1,$2,$3,$4,$5,$6,$7)',[org.id,JSON.stringify(snapshot.state.bindings),JSON.stringify({kind:'offline-neon-retained-organization',snapshotSha256:snapshot.sha256,preservedMemberships:true,preservedGrants:true}),input.evidenceRef,input.evidenceSha256,input.operatorSubject,input.archiveEnvelope]);
   const after=await inspectRetainedNeonOrganization(db,org.id);if(after.sha256!==snapshot.sha256)throw Error('Organization data changed during preservation cutover');
  }
  for(const {item,snapshot} of archivedUsers){
   const {target}=item;
   await db.query('INSERT INTO webdock_auth.neon_identity(user_id,provider_origin,provider_subject,linked_by) VALUES($1,$2,$3::uuid,$4)',[target.canonicalId,target.providerOrigin,target.providerSubject,input.operatorSubject]);
   await db.query('UPDATE webdock_auth.clerk_identity SET enabled=false WHERE user_id=$1',[target.canonicalId]);
   await db.query('UPDATE webdock_auth.clerk_identity_mapping SET revoked_at=coalesce(revoked_at,statement_timestamp()) WHERE local_subject=$1',[target.canonicalId]);
   await db.query('INSERT INTO webdock_auth.local_identity_activation(user_id,bindings,approved_state,evidence_ref,evidence_sha256,operator_subject,archive_envelope) VALUES($1,$2,$3,$4,$5,$6,$7)',[target.canonicalId,JSON.stringify(snapshot.state.bindings),JSON.stringify({kind:'offline-neon-reset-first-retained-user',providerSubject:target.providerSubject,snapshotSha256:snapshot.sha256,preservedCredentials:true,preservedFactors:true,preservedMemberships:true,preservedGrants:true}),input.evidenceRef,input.evidenceSha256,input.operatorSubject,input.archiveEnvelope]);
   // CI/CD tokens keep their independent, scoped sessions; stale interactive sessions do not.
   await db.query('DELETE FROM webdock_auth.session s WHERE "userId"=$1 AND NOT EXISTS(SELECT 1 FROM webdock_auth.api_token t WHERE t.session_id=s.id)',[target.canonicalId]);
   if((await inspectRetainedNeonUser(db,target.canonicalId)).sha256!==snapshot.sha256)throw Error('Native identity, factors or rights changed');
   if(!(await db.query('SELECT webdock_rls.local_identity_authority($1) AS allowed',[target.canonicalId])).rows[0]?.allowed)throw Error('Reviewed identity activation did not become valid');
  }
  for(const org of input.organizations)if(!(await db.query('SELECT webdock_rls.local_organization_authority($1) AS allowed',[org.id])).rows[0]?.allowed)throw Error('Reviewed organization activation did not become valid');
  await db.query('COMMIT');return{users:input.users.length,organizations:input.organizations.length,canonicalIdsRetained:true,passwordsFactorsMembershipsGrantsPreserved:true,bulkEmailSent:false};
 }catch(error){await db.query('ROLLBACK');throw error;}
}
