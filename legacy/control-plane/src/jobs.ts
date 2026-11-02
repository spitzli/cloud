import { randomUUID } from "node:crypto";
import type { Pool } from "pg";
type DB = Pick<Pool, "query">;
export const jobNames = [
  "hosting.setup",
  "storage.refresh",
  "mail.dispatch",
  "mail.events",
  "mail.webhooks",
  "mail.maintenance",
] as const;
export type JobName = (typeof jobNames)[number];
export type ClaimedJob = {
  id: string;
  name: JobName;
  claimToken: string;
  attempts: number;
};
export async function schedule(
  db: DB,
  name: JobName,
  slot: Date,
): Promise<void> {
  if (!jobNames.includes(name) || !Number.isFinite(slot.getTime()))
    throw Error("Invalid scheduled job");
  await db.query(
    "INSERT INTO webdock_auth.api_job(name,slot,available_at) VALUES($1,$2,$2) ON CONFLICT(name,slot) DO NOTHING",
    [name, slot],
  );
}
export async function claim(
  db: DB,
  names: readonly JobName[] = jobNames,
): Promise<ClaimedJob | null> {
  if (!names.length || names.some((name) => !jobNames.includes(name)))
    throw Error("Invalid worker ownership");
  const token = randomUUID();
  const row = (
    await db.query(
      `WITH next AS (
  SELECT id FROM webdock_auth.api_job WHERE name=ANY($2::text[]) AND available_at<=now() AND (status='pending' OR (status='running' AND lease_until<now()))
  ORDER BY slot,id FOR UPDATE SKIP LOCKED LIMIT 1
 ) UPDATE webdock_auth.api_job j SET status='running',attempts=attempts+1,claim_token=$1,lease_until=now()+interval '60 seconds'
 FROM next WHERE j.id=next.id RETURNING j.id,j.name,j.attempts`,
      [token, names],
    )
  ).rows[0];
  return row ? { ...row, claimToken: token } : null;
}
export async function complete(db: DB, job: ClaimedJob): Promise<boolean> {
  return (
    (
      await db.query(
        "UPDATE webdock_auth.api_job SET status='succeeded',finished_at=now(),lease_until=NULL WHERE id=$1 AND claim_token=$2 AND status='running' AND lease_until>now()",
        [job.id, job.claimToken],
      )
    ).rowCount === 1
  );
}
export async function renew(db: DB, job: ClaimedJob): Promise<boolean> {
  return (
    (
      await db.query(
        "UPDATE webdock_auth.api_job SET lease_until=now()+interval '60 seconds' WHERE id=$1 AND claim_token=$2 AND status='running' AND lease_until>now()",
        [job.id, job.claimToken],
      )
    ).rowCount === 1
  );
}
export async function fail(db: DB, job: ClaimedJob): Promise<void> {
  await db.query(
    "UPDATE webdock_auth.api_job SET status=CASE WHEN attempts>=8 THEN 'failed' ELSE 'pending' END,available_at=now()+interval '1 minute',lease_until=NULL WHERE id=$1 AND claim_token=$2 AND status='running' AND lease_until>now()",
    [job.id, job.claimToken],
  );
}
