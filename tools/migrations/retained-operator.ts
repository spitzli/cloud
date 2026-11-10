/** First retained operator only: offline custody + reviewed bindings + actual existing factors.
 * No HTTP route imports this module. It creates no user, password, factor or passkey.
 */
import {createHash,timingSafeEqual} from 'node:crypto';
import {readFile,readdir} from 'node:fs/promises';
import {Pool,type PoolClient} from 'pg';
import {z} from 'zod';
import {symmetricDecrypt,symmetricEncrypt,verifyPassword} from 'better-auth/crypto';
import {createOTP} from '@better-auth/utils/otp';
import {createClerkClient} from '@clerk/backend';
import {verifiedPostgresConfig} from '@spitzli/database-runtime';
import {migrationConfig} from './config';

type Target={subject:string;email:string;issuer:string;externalSubject:string};
type ProviderEvidence={issuer:string;externalSubject:string;email:string;emailVerified:boolean;operator:boolean;banned:boolean;locked:boolean;checkedAt:number};
export function validateRetainedOperatorEvidence(t:Target,p:ProviderEvidence){
 if(p.issuer!==t.issuer||p.externalSubject!==t.externalSubject||typeof p.email!=='string'||p.email.toLowerCase()!==t.email.toLowerCase()||p.emailVerified!==true||p.operator!==true||p.banned!==false||p.locked!==false||!Number.isFinite(p.checkedAt)||Date.now()-p.checkedAt>300000||p.checkedAt>Date.now()+30000)
  throw Error('Current exact provider ownership and existing operator role must be verified');
}
const id=z.string().regex(/^[1-9][0-9]{0,18}$/);
const targetSchema=z.object({subject:id,email:z.email(),issuer:z.url().refine(s=>{const u=new URL(s);return u.protocol==='https:'&&!u.username&&!u.password&&!u.search&&!u.hash&&u.pathname==='/';}),externalSubject:z.string().regex(/^user_[a-zA-Z0-9]+$/)}).strict();
const approvalSchema=z.object({target:targetSchema,snapshotSha256:z.string().regex(/^[a-f0-9]{64}$/),evidenceRef:z.string().min(1).max(1024),evidenceSha256:z.string().regex(/^[a-f0-9]{64}$/),keepMemberships:z.array(z.object({id,organizationId:id,role:z.enum(['owner','admin','member'])}).strict()).max(100),keepExternalOwnership:z.array(z.object({id,organizationId:id,role:z.literal('owner')}).strict()).max(100).default([]),keepGrantIds:z.array(id).max(100)}).strict();
type Approval=z.infer<typeof approvalSchema>;
const digest=(s:string)=>createHash('sha256').update(s).digest('hex');
function canonical(v:any):string{return JSON.stringify(sort(v));}
function sort(v:any):any{return Array.isArray(v)?v.map(sort):v&&typeof v==='object'?Object.fromEntries(Object.keys(v).sort().map(k=>[k,sort(v[k])])):v;}
const tables=['organization','user','account','twoFactor','passkey','clerk_identity','clerk_identity_mapping','clerk_session_mapping','clerk_organization','local_identity_activation','local_organization_activation','member','project_grant','session','oauthAccessToken','oauthRefreshToken','identity_mfa_challenge','identity_recovery_token','identity_delivery_job','identity_passkey_ceremony','identity_totp_replay','identity_browser_session','identity_oauth_request','identity_mfa_enrollment','api_token','access_delivery_job','database_launch','database_session','managed_database_launch','managed_database_session','studio_tenant_preview'];
const q=(name:string)=>'"'+name.replaceAll('"','""')+'"';

async function guarded(db:PoolClient,expected:string){
 const r=(await db.query("SELECT current_database() AS database,session_user AS role,current_user AS active,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=session_user")).rows[0];
 if(r?.database!==expected||r.role!=='webdock_offline_migrate'||r.active!==r.role||r.rolsuper!==false||r.rolbypassrls!==false)throw Error('Exact database and dedicated non-superuser offline role required');
 const directory=new URL('../../crates/persistence/migrations/',import.meta.url);
 const expectedRows=new Map(await Promise.all((await readdir(directory)).filter(n=>/^\d+_[a-z0-9_]+\.sql$/.test(n)).map(async n=>[String(Number(n.split('_')[0])),createHash('sha384').update(await readFile(new URL(n,directory))).digest('hex')] as const)));
 const rows=(await db.query("SELECT version::text AS version,encode(checksum,'hex') AS checksum,success FROM webdock_api_migrations.rust_journal ORDER BY version")).rows;
 if(rows.length!==expectedRows.size||rows.some(r=>r.success!==true||expectedRows.get(r.version)!==r.checksum))throw Error('Exact completed native schema required');
}
async function snapshot(db:PoolClient,subject:string){
 const user=(await db.query('SELECT to_jsonb(u) AS value FROM webdock_auth."user" u WHERE id=$1',[subject])).rows;
 if(user.length!==1)throw Error('Existing operator subject not found');
 const state:any={user:user[0].value};
 for(const [key,sql] of [
  ['legacyLinks','SELECT to_jsonb(t) AS value FROM webdock_auth.clerk_identity t WHERE user_id=$1 ORDER BY issuer,clerk_user_id'],
  ['mappingLinks','SELECT to_jsonb(t) AS value FROM webdock_auth.clerk_identity_mapping t WHERE local_subject=$1 ORDER BY issuer,external_subject'],
  ['bindings',"SELECT jsonb_build_array(source,issuer,external_subject) AS value FROM (SELECT 'legacy'::text AS source,issuer,clerk_user_id AS external_subject FROM webdock_auth.clerk_identity WHERE user_id=$1 UNION ALL SELECT 'mapping',issuer,external_subject FROM webdock_auth.clerk_identity_mapping WHERE local_subject=$1) x ORDER BY source,issuer,external_subject"],
  ['accounts','SELECT to_jsonb(t) AS value FROM webdock_auth.account t WHERE "userId"=$1 ORDER BY id'],
  ['factors','SELECT to_jsonb(t) AS value FROM webdock_auth."twoFactor" t WHERE "userId"=$1 ORDER BY id'],
  ['passkeys','SELECT to_jsonb(t) AS value FROM webdock_auth.passkey t WHERE "userId"=$1 ORDER BY id'],
  ['sessions','SELECT to_jsonb(t) AS value FROM webdock_auth.session t WHERE "userId"=$1 ORDER BY id'],
  ['clerkSessions','SELECT to_jsonb(t) AS value FROM webdock_auth.clerk_session_mapping t WHERE local_session_id IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1) ORDER BY local_session_id'],
  ['databaseLaunches','SELECT to_jsonb(t) AS value FROM webdock_auth.database_launch t WHERE session_id IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1) ORDER BY token_hash'],
  ['databaseSessions','SELECT to_jsonb(t) AS value FROM webdock_auth.database_session t WHERE session_id IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1) ORDER BY id'],
  ['managedDatabaseLaunches','SELECT to_jsonb(t) AS value FROM webdock_auth.managed_database_launch t WHERE session_id IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1) ORDER BY token_hash'],
  ['managedDatabaseSessions','SELECT to_jsonb(t) AS value FROM webdock_auth.managed_database_session t WHERE session_id IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1) ORDER BY id'],
  ['tenantPreviews','SELECT to_jsonb(t) AS value FROM webdock_auth.studio_tenant_preview t WHERE session_id IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1) ORDER BY session_id'],
  ['webmailLaunches','SELECT to_jsonb(t) AS value FROM webdock_mail.webmail_launch t WHERE session_id IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1) ORDER BY token_hash'],
  ['webmailSessions','SELECT to_jsonb(t) AS value FROM webdock_mail.webmail_session t WHERE session_id IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1) ORDER BY id'],
  ['memberships','SELECT to_jsonb(t) AS value FROM webdock_auth.member t WHERE "userId"=$1 ORDER BY id'],
  ['grants','SELECT to_jsonb(t) AS value FROM webdock_auth.project_grant t WHERE user_id=$1 ORDER BY id'],
  ['oauthAccess','SELECT to_jsonb(t) AS value FROM webdock_auth."oauthAccessToken" t WHERE "userId"=$1 OR "sessionId" IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1) ORDER BY id'],
  ['oauthRefresh','SELECT to_jsonb(t) AS value FROM webdock_auth."oauthRefreshToken" t WHERE "userId"=$1 OR "sessionId" IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1) ORDER BY id'],
  ['pendingMfa','SELECT to_jsonb(t) AS value FROM webdock_auth.identity_mfa_challenge t WHERE user_id=$1 ORDER BY token_hash'],
  ['pendingRecovery','SELECT to_jsonb(t) AS value FROM webdock_auth.identity_recovery_token t WHERE user_id=$1 ORDER BY token_hash'],
  ['pendingDelivery','SELECT to_jsonb(t) AS value FROM webdock_auth.identity_delivery_job t WHERE token_hash IN (SELECT token_hash FROM webdock_auth.identity_recovery_token WHERE user_id=$1) ORDER BY id'],
  ['passkeyCeremonies','SELECT to_jsonb(t) AS value FROM webdock_auth.identity_passkey_ceremony t WHERE user_id=$1 OR session_id IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1) ORDER BY token_hash'],
  ['browserSessions','SELECT to_jsonb(t) AS value FROM webdock_auth.identity_browser_session t WHERE user_id=$1 ORDER BY token_hash'],
  ['oauthRequests','SELECT to_jsonb(t) AS value FROM webdock_auth.identity_oauth_request t WHERE user_id=$1 ORDER BY token_hash'],
  ['mfaEnrollments','SELECT to_jsonb(t) AS value FROM webdock_auth.identity_mfa_enrollment t WHERE session_id IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1) ORDER BY session_id'],
  ['apiTokens','SELECT to_jsonb(t) AS value FROM webdock_auth.api_token t WHERE user_id=$1 ORDER BY id'],
  ['accessDeliveries','SELECT to_jsonb(t) AS value FROM webdock_auth.access_delivery_job t WHERE actor_id=$1 ORDER BY id'],
  ['replay','SELECT to_jsonb(t) AS value FROM webdock_auth.identity_totp_replay t WHERE user_id=$1'],
 ] as const)state[key]=(await db.query(sql,[subject])).rows.map(r=>r.value);
 return state;
}
export async function inspectRetainedOperator(db:PoolClient,expected:string,target:Target){
 targetSchema.parse(target);await guarded(db,expected);
 const state=await snapshot(db,target.subject);
 const nativeOrgIds=new Set((await db.query('SELECT id FROM webdock_auth.organization WHERE webdock_rls.local_organization_authority(id)')).rows.map(r=>r.id));
 const keepExternalOwnership=[];
 for(const m of state.memberships.filter((m:any)=>m.role==='owner'&&!nativeOrgIds.has(m.organizationId))){const other=(await db.query('SELECT count(*)::int AS n FROM webdock_auth.member WHERE "organizationId"=$1 AND "userId"<>$2 AND role=\'owner\'',[m.organizationId,target.subject])).rows[0];if(other.n===0)keepExternalOwnership.push({id:m.id,organizationId:m.organizationId,role:'owner' as const});}
 return {target,snapshotSha256:digest(canonical(state)),bindings:state.bindings,keepMemberships:state.memberships.filter((m:any)=>nativeOrgIds.has(m.organizationId)).map((m:any)=>({id:m.id,organizationId:m.organizationId,role:m.role})),keepExternalOwnership,keepGrantIds:state.grants.filter((g:any)=>nativeOrgIds.has(g.organization_id)).map((g:any)=>g.id)};
}

export async function adoptRetainedOperator(db:PoolClient,expected:string,authSecret:string,approval:Approval,password:string,code:string,provider:ProviderEvidence){
 approval=approvalSchema.parse(approval);validateRetainedOperatorEvidence(approval.target,provider);
 if(authSecret.length<32||password.length>4096||!/^\d{6}$/.test(code))throw Error('Existing password and unused current MFA proof required');
 await db.query('BEGIN');
 try{
  await db.query("SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='30s'");await guarded(db,expected);
  await db.query("SELECT pg_advisory_xact_lock(hashtext('webdock-first-operator'))");
  // Table locks also prevent insert phantoms from webhooks, invitations and parallel setup.
  await db.query('LOCK TABLE '+[...tables.map(t=>'webdock_auth.'+q(t)),'webdock_mail.webmail_launch','webdock_mail.webmail_session'].join(',')+' IN SHARE ROW EXCLUSIVE MODE');
  const native=(await db.query("SELECT count(*)::int AS n FROM webdock_auth.\"user\" WHERE role='operator' AND webdock_rls.local_identity_authority(id)")).rows[0];
  if(native.n!==0)throw Error('A locally authoritative operator already exists; use normal reviewed administration');
  const {subject,email,issuer,externalSubject}=approval.target;
  const state=await snapshot(db,subject);
  if(digest(canonical(state))!==approval.snapshotSha256)throw Error('Retained operator inventory changed; new review required');
  const u=state.user;
  if(u.id!==subject||u.email.toLowerCase()!==email.toLowerCase()||u.role!=='operator'||u.emailVerified!==true||u.twoFactorEnabled!==true||u.mustChangePassword!==false||u.banned===true)throw Error('Existing verified and fully secured operator required');
  if(!state.bindings.length||state.bindings.some((b:string[])=>!['legacy','mapping'].includes(b[0]!)||b[1]!==issuer||b[2]!==externalSubject))throw Error('Unexpected retained external binding');
  if((await db.query('SELECT user_id FROM webdock_auth.local_identity_activation WHERE user_id=$1',[subject])).rowCount)throw Error('Operator activation already exists');
  const credentials=state.accounts.filter((a:any)=>a.providerId==='credential');
  if(credentials.length!==1||typeof credentials[0].password!=='string'||!await verifyPassword({hash:credentials[0].password,password}))throw Error('Existing operator password rejected');
  if(state.factors.length!==1)throw Error('Exactly one usable retained MFA factor required');
  const factor=state.factors[0];const now=Number((await db.query('SELECT extract(epoch FROM statement_timestamp())::bigint AS now')).rows[0].now);
  if(factor.verified===false||(factor.lockedUntil&&Date.parse(factor.lockedUntil)>now*1000))throw Error('Retained factor is unverified or locked');
  const secret=await symmetricDecrypt({key:authSecret,data:factor.secret});const otp=createOTP(secret);let step:number|undefined;
  for(const candidate of [Math.floor(now/30),Math.floor(now/30)-1,Math.floor(now/30)+1])if(timingSafeEqual(Buffer.from(await otp.hotp(candidate)),Buffer.from(code))){step=candidate;break;}
  if(step===undefined)throw Error('Existing current MFA proof rejected');
  const replay=state.replay[0];const secretDigest=digest(secret);
  if(replay&&(replay.secret_digest!==secretDigest||Number(replay.last_step)>=step))throw Error('MFA replay or unreviewed factor change rejected');
  const keep=new Set([...approval.keepMemberships,...approval.keepExternalOwnership].map(m=>m.id));if(keep.size!==approval.keepMemberships.length+approval.keepExternalOwnership.length)throw Error('Duplicate approved membership');
  for(const m of approval.keepMemberships){const old=state.memberships.find((x:any)=>x.id===m.id);const local=(await db.query('SELECT webdock_rls.local_organization_authority($1) AS allowed',[m.organizationId])).rows[0]?.allowed;if(!old||old.organizationId!==m.organizationId||old.role!==m.role||local!==true)throw Error('Approved membership is not exact existing native authority');}
  // Preserve an explicit last external ownership pointer without approving that
  // organization or granting native member authority. Its separate migration
  // still requires the ordinary reviewed native-owner manifest.
  for(const m of approval.keepExternalOwnership){const old=state.memberships.find((x:any)=>x.id===m.id);const local=(await db.query('SELECT webdock_rls.local_organization_authority($1) AS allowed',[m.organizationId])).rows[0]?.allowed;const other=(await db.query('SELECT count(*)::int AS n FROM webdock_auth.member WHERE "organizationId"=$1 AND "userId"<>$2 AND role=\'owner\'',[m.organizationId,subject])).rows[0];if(!old||old.organizationId!==m.organizationId||old.role!=='owner'||local!==false||other.n!==0)throw Error('External ownership pointer is not an exact reviewed sole owner');}
  for(const old of state.memberships.filter((m:any)=>!keep.has(m.id)&&['owner','admin'].includes(m.role))){const remaining=(await db.query('SELECT count(*)::int AS n FROM webdock_auth.member m JOIN webdock_auth."user" u ON u.id=m."userId" WHERE m."organizationId"=$1 AND m."userId"<>$2 AND m.role=\'owner\' AND NOT coalesce(u.banned,false)',[old.organizationId,subject])).rows[0];if(remaining.n<1)throw Error('Unapproved removal would strand an organization');}
  if(new Set(approval.keepGrantIds).size!==approval.keepGrantIds.length)throw Error('Duplicate approved grant');
  for(const g of approval.keepGrantIds){const old=state.grants.find((x:any)=>x.id===g);if(!old||(await db.query('SELECT webdock_rls.local_organization_authority($1) AS allowed',[old.organization_id])).rows[0]?.allowed!==true)throw Error('Approved grant is not existing native authority');}
  const archive=await symmetricEncrypt({key:{currentVersion:1,keys:new Map([[1,authSecret]]),legacySecret:authSecret},data:canonical(state)});
  await db.query('INSERT INTO webdock_auth.local_identity_activation(user_id,bindings,approved_state,evidence_ref,evidence_sha256,operator_subject,archive_envelope) VALUES($1,$2,$3,$4,$5,$1,$6)',[subject,JSON.stringify(state.bindings),JSON.stringify({kind:'offline-first-retained-operator',role:'operator',credentials:'verified-retained',memberships:approval.keepMemberships,externalOwnershipPointers:approval.keepExternalOwnership,grants:approval.keepGrantIds,provider,inventorySha256:approval.snapshotSha256}),approval.evidenceRef,approval.evidenceSha256,archive]);
  await db.query('UPDATE webdock_auth.clerk_identity SET enabled=false,legacy_eligible=false WHERE user_id=$1',[subject]);
  await db.query('UPDATE webdock_auth.clerk_identity_mapping SET revoked_at=coalesce(revoked_at,statement_timestamp()) WHERE local_subject=$1',[subject]);
  await db.query('DELETE FROM webdock_auth.member WHERE "userId"=$1 AND NOT(id=ANY($2::text[]))',[subject,[...keep]]);
  await db.query('UPDATE webdock_auth.project_grant SET enabled=false WHERE user_id=$1 AND NOT(id=ANY($2::text[]))',[subject,approval.keepGrantIds]);
  await db.query('UPDATE webdock_auth."oauthAccessToken" SET revoked=coalesce(revoked,statement_timestamp()) WHERE "userId"=$1 OR "sessionId" IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1)',[subject]);
  await db.query('UPDATE webdock_auth."oauthRefreshToken" SET revoked=coalesce(revoked,statement_timestamp()) WHERE "userId"=$1 OR "sessionId" IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1)',[subject]);
  await db.query('DELETE FROM webdock_auth.identity_passkey_ceremony WHERE user_id=$1 OR session_id IN (SELECT id FROM webdock_auth.session WHERE "userId"=$1)',[subject]);
  for(const sql of ['DELETE FROM webdock_auth.identity_mfa_challenge WHERE user_id=$1','DELETE FROM webdock_auth.identity_recovery_token WHERE user_id=$1','DELETE FROM webdock_auth.identity_oauth_request WHERE user_id=$1','DELETE FROM webdock_auth.session WHERE "userId"=$1'])await db.query(sql,[subject]);
  await db.query("UPDATE webdock_auth.access_delivery_job SET state='canceled',lease_until=NULL WHERE actor_id=$1 AND state='queued'",[subject]);
  const advanced=await db.query('INSERT INTO webdock_auth.identity_totp_replay(user_id,secret_digest,last_step) VALUES($1,$2,$3) ON CONFLICT(user_id) DO UPDATE SET last_step=excluded.last_step WHERE identity_totp_replay.secret_digest=excluded.secret_digest AND identity_totp_replay.last_step<excluded.last_step RETURNING user_id',[subject,secretDigest,step]);if(advanced.rowCount!==1)throw Error('MFA replay advancement failed');
  const after=await snapshot(db,subject);
  for(const key of ['user','accounts','factors','passkeys'])if(canonical(after[key])!==canonical(state[key]))throw Error('Retained credentials or security flags changed');
  if(!(await db.query('SELECT webdock_rls.local_identity_authority($1) AS allowed',[subject])).rows[0]?.allowed)throw Error('Native authority did not become valid');
  await db.query('COMMIT');return {subject,role:'operator',credentialsRetained:true,mfaRetained:true,passkeysRetained:true,bootstrapOtpStep:step,nativeAuthority:true};
 }catch(error){await db.query('ROLLBACK');throw error;}
}

if(import.meta.main){
 const config=migrationConfig(process.env,process.argv[2]);if(config.bootstrap)throw Error('Existing operator adoption never initializes a database');
 const inputText=await Bun.stdin.text();if(Buffer.byteLength(inputText)>1048576)throw Error('Bounded private input required');const input=JSON.parse(inputText);
 const target=targetSchema.parse(input.target??input.approval?.target);const pool=new Pool({...config.local?{connectionString:config.url.href}:verifiedPostgresConfig(config.url.href,config.ca),max:1});
 try{const db=await pool.connect();try{
  if(input.mode==='inspect')console.log(JSON.stringify(await inspectRetainedOperator(db,decodeURIComponent(config.url.pathname.slice(1)),target)));
  else{const key=process.env.CLERK_SECRET_KEY;const issuer=process.env.CLERK_ISSUER;const auth=process.env.WEBDOCK_AUTH_SECRET;if(!key||!issuer||!auth)throw Error('Existing provider and Auth keys required');const client=createClerkClient({secretKey:key});const user=await client.users.getUser(target.externalSubject);const email=user.emailAddresses.find(e=>e.id===user.primaryEmailAddressId);const provider:ProviderEvidence={issuer,externalSubject:user.id,email:email?.emailAddress??'',emailVerified:email?.verification?.status==='verified',operator:user.privateMetadata.webdockRole==='operator',banned:user.banned,locked:user.locked,checkedAt:Date.now()};console.log(JSON.stringify(await adoptRetainedOperator(db,decodeURIComponent(config.url.pathname.slice(1)),auth,input.approval,input.password,input.code,provider)));}
 }finally{db.release();}}catch(error){console.error(error instanceof Error&&!('code'in error)?error.message:'Retained operator adoption failed; transaction rolled back ('+String((error as {code?:string}).code??'unknown')+')');process.exitCode=1;}finally{await pool.end();}
}
