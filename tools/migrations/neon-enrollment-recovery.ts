/** Offline owner repair only. Never import this module from an HTTP/runtime handler. */
import type { PoolClient } from "pg";
import { z } from "zod";
import { snapshotDigest } from "./retained-neon";

export class NeonRecoveryError extends Error {}
function reject(message: string): never { throw new NeonRecoveryError(message); }
const digest = z.string().regex(/^[a-f0-9]{64}$/);
const reference = z.string().min(1).max(1024).refine(value => !/\p{Cc}/u.test(value));
const providerOrigin = z.string().refine(value => {
  try {
    const u = new URL(value);
    return value === u.href && u.protocol === "https:" && !u.username && !u.password && !u.port && !u.search && !u.hash
      && u.hostname.includes(".neonauth.") && u.hostname.endsWith(".neon.tech") && /^\/[^/]+\/auth$/.test(u.pathname);
  } catch { return false; }
});
const targetSchema = z.object({ userID: z.string().regex(/^[1-9][0-9]{0,18}$/), providerSubject: z.uuid(), providerOrigin }).strict();
const reviewSchema = targetSchema.extend({ snapshotSha256: digest, evidenceRef: reference, evidenceSha256: digest, reviewerRef: reference }).strict();
type Target = z.infer<typeof targetSchema>;
type Credential = { digest: string; usable: boolean };
type State = {
  core: null | { id: string; email: string; role: string; verified: boolean; banned: boolean; local: boolean; mfa: boolean; mustChangePassword: boolean; credentials: Credential[] };
  provider: null | { id: string; email: string; verified: boolean; banned: boolean; credentials: Credential[] };
  mappings: { user_id: string; provider_origin: string; provider_subject: string; enabled: boolean; revision: number; linked_by: string; linked_at: string }[];
};
function input<T extends Target>(schema: z.ZodType<T>, value: unknown, expectedOrigin: string): T {
  const result = schema.safeParse(value);
  if (!result.success || !providerOrigin.safeParse(expectedOrigin).success || result.data.providerOrigin !== expectedOrigin)
    reject("Exact reviewed identifiers and the configured Managed origin are required");
  return result.data;
}
export function neonRecoveryConfig(env: Record<string, string | undefined>, target: string | undefined) {
  let url: URL;
  try { url = new URL(env.DATABASE_URL_UNPOOLED ?? ""); } catch { return reject("Explicit offline DATABASE_URL_UNPOOLED required"); }
  if (!["postgres:", "postgresql:"].includes(url.protocol) || decodeURIComponent(url.username) !== "neondb_owner" || !url.hostname || url.hash || url.search)
    reject("Use the explicit offline neondb_owner connection");
  url.port ||= "5432";
  if (target !== `${url.hostname}:${url.port}${url.pathname}`) reject("Pass the exact recovery host:port/database");
  const local = ["127.0.0.1", "localhost"].includes(url.hostname);
  if (!local && !env.DATABASE_CA_CERT) reject("Remote recovery requires a verified CA");
  if (!providerOrigin.safeParse(env.NEON_AUTH_BASE_URL).success) reject("Pin the exact configured NEON_AUTH_BASE_URL");
  return { url, local, ca: env.DATABASE_CA_CERT, database: decodeURIComponent(url.pathname.slice(1)), providerOrigin: env.NEON_AUTH_BASE_URL! };
}
async function owner(db: PoolClient, expectedDatabase: string) {
  const r = (await db.query("SELECT session_user AS role,current_user AS active,current_database() AS database")).rows[0];
  if (!expectedDatabase || r?.role !== "neondb_owner" || r.active !== r.role || r.database !== expectedDatabase)
    reject("Exact database and explicit offline neondb_owner login required");
  await db.query("SET LOCAL ROLE webdock_platform_owner");
  const assumed = (await db.query("SELECT current_user AS active,r.rolcanlogin,r.rolsuper,r.rolbypassrls FROM pg_catalog.pg_roles r WHERE r.rolname=current_user")).rows[0];
  if (assumed?.active !== "webdock_platform_owner" || assumed.rolcanlogin !== false || assumed.rolsuper !== false || assumed.rolbypassrls !== false)
    reject("Existing restricted NOLOGIN platform owner role required");
}
// Password values remain inside PostgreSQL. Only one-way fingerprints enter the review snapshot.
async function snapshot(db: PoolClient, target: Target): Promise<State> {
  return (await db.query(`SELECT jsonb_build_object(
    'core',(SELECT jsonb_build_object('id',u.id,'email',u.email,'role',u.role,'verified',u."emailVerified",'banned',coalesce(u.banned,false),'local',webdock_rls.local_identity_authority(u.id),'mfa',coalesce(u."twoFactorEnabled",false),'mustChangePassword',u."mustChangePassword",'credentials',
      coalesce((SELECT jsonb_agg(jsonb_build_object('id',a.id,'digest',encode(sha256(convert_to(coalesce(a.password,''),'UTF8')),'hex'),'usable',a.password IS NOT NULL AND a.password<>'') ORDER BY a.id) FROM webdock_auth.account a WHERE a."userId"=u.id AND a."providerId"='credential'),'[]'::jsonb)) FROM webdock_auth."user" u WHERE u.id=$1),
    'provider',(SELECT jsonb_build_object('id',u.id,'email',u.email,'verified',u."emailVerified",'banned',coalesce(u.banned,false),'credentials',
      coalesce((SELECT jsonb_agg(jsonb_build_object('providerId',a."providerId",'digest',encode(sha256(convert_to(coalesce(a.password,''),'UTF8')),'hex'),'usable',a.password IS NOT NULL AND a.password<>'') ORDER BY a."providerId",a.password) FROM neon_auth.account a WHERE a."userId"=u.id AND a."providerId"='credential'),'[]'::jsonb)) FROM neon_auth."user" u WHERE u.id=$2::uuid),
    'mappings',coalesce((SELECT jsonb_agg(to_jsonb(n) ORDER BY n.user_id) FROM webdock_auth.neon_identity n WHERE n.user_id=$1 OR n.provider_subject=$2::uuid),'[]'::jsonb)
  ) AS state`, [target.userID, target.providerSubject])).rows[0].state;
}
function eligible(state: State, target: Target) {
  const { core, provider } = state;
  if (!core || !provider || core.id !== target.userID || provider.id !== target.providerSubject || !core.local || !core.verified || core.banned
    || !["user", "operator"].includes(core.role) || provider.banned || core.email.toLowerCase() !== provider.email.toLowerCase()
    || [core.credentials, provider.credentials].some(values => values.length !== 1 || !values[0].usable || !digest.safeParse(values[0].digest).success))
    reject("Exact approved Core identity and usable reviewed provider identity required");
}
export async function inspectNeonEnrollmentRecovery(db: PoolClient, expectedDatabase: string, expectedOrigin: string, value: unknown) {
  const target = input(targetSchema, value, expectedOrigin);
  await db.query("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY");
  try {
    await owner(db, expectedDatabase);
    await db.query("SET LOCAL timezone='UTC'; SET LOCAL search_path=pg_catalog");
    const state = await snapshot(db, target);
    eligible(state, target);
    await db.query("COMMIT");
    return { ...target, state, snapshotSha256: snapshotDigest(state) };
  } catch (error) { await db.query("ROLLBACK").catch(() => {}); throw error; }
}
export async function recoverNeonEnrollment(db: PoolClient, expectedDatabase: string, expectedOrigin: string, value: unknown) {
  const review = input(reviewSchema, value, expectedOrigin);
  await db.query("BEGIN ISOLATION LEVEL SERIALIZABLE");
  try {
    await owner(db, expectedDatabase);
    await db.query("SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='30s'; SET LOCAL timezone='UTC'; SET LOCAL search_path=pg_catalog");
    await db.query('SELECT id FROM webdock_auth."user" WHERE id=$1 FOR UPDATE', [review.userID]);
    await db.query('SELECT id FROM webdock_auth.account WHERE "userId"=$1 AND "providerId"=\'credential\' ORDER BY id FOR UPDATE', [review.userID]);
    // Provider facts share this SERIALIZABLE snapshot; recovery requires no provider write privileges.
    await db.query("LOCK TABLE webdock_auth.neon_identity,webdock_auth.neon_enrollment_recovery IN SHARE ROW EXCLUSIVE MODE");
    const state = await snapshot(db, review);
    eligible(state, review);
    const audits = (await db.query("SELECT * FROM webdock_auth.neon_enrollment_recovery WHERE user_id=$1 OR provider_subject=$2::uuid ORDER BY user_id", [review.userID, review.providerSubject])).rows;
    let status: "recovered" | "already-recovered" = "recovered";
    if (state.mappings.length || audits.length) {
      const audit = audits[0], mapping = state.mappings[0];
      if (audits.length !== 1 || state.mappings.length !== 1 || !mapping.enabled || mapping.revision !== 1
        || mapping.user_id !== review.userID || mapping.provider_subject !== review.providerSubject || mapping.provider_origin !== review.providerOrigin
        || audit.user_id !== review.userID || audit.provider_subject !== review.providerSubject || audit.provider_origin !== review.providerOrigin
        || audit.snapshot_sha256 !== review.snapshotSha256 || audit.evidence_ref !== review.evidenceRef || audit.evidence_sha256 !== review.evidenceSha256
        || audit.reviewer_ref !== review.reviewerRef || audit.owner_role !== "neondb_owner"
        || audit.mapping_sha256 !== snapshotDigest(state.mappings) || snapshotDigest({ ...state, mappings: [] }) !== review.snapshotSha256)
        reject("Existing, rebound or changed recovery state requires a new explicit investigation");
      status = "already-recovered";
    } else {
      if (snapshotDigest(state) !== review.snapshotSha256) reject("Reviewed identity or credential snapshot changed");
      await db.query("INSERT INTO webdock_auth.neon_identity(user_id,provider_origin,provider_subject,linked_by) VALUES($1,$2,$3::uuid,'offline-owner:neondb_owner')", [review.userID, review.providerOrigin, review.providerSubject]);
      const bound = await snapshot(db, review);
      if (snapshotDigest({ ...bound, mappings: [] }) !== review.snapshotSha256) reject("Identity changed during recovery");
      await db.query("INSERT INTO webdock_auth.neon_enrollment_recovery(user_id,provider_origin,provider_subject,snapshot_sha256,mapping_sha256,evidence_ref,evidence_sha256,reviewer_ref,owner_role) VALUES($1,$2,$3::uuid,$4,$5,$6,$7,$8,session_user)", [review.userID, review.providerOrigin, review.providerSubject, review.snapshotSha256, snapshotDigest(bound.mappings), review.evidenceRef, review.evidenceSha256, review.reviewerRef]);
    }
    await db.query("COMMIT");
    return { status, userID: review.userID, providerSubject: review.providerSubject, sessionIssued: false };
  } catch (error) { await db.query("ROLLBACK").catch(() => {}); throw error; }
}
