import { activeIdentity, type IdentityDirectory } from "@spitzli/core/identity";
import { randomUUID } from "node:crypto";
import type { Pool, PoolClient } from "pg";
import { reserveMailCapacity, completeMailCapacity, MailCapacityError } from "./capacity.ts";
export type MailService = { customerID: string; enabled: boolean; state: "disabled" | "pending" | "provisioning" | "ready" | "suspended" | "needs_review"; revision: string };
export type MailOperation = { id: string; customerID: string; revision: string; enabled: boolean; instanceKey: string; hostID: string; leaseToken: string; leaseUntil?: string };
export class MailServiceError extends Error {}
const fail = (message: string): never => { throw new MailServiceError(message); };
function customerID(value: string) {
  if (!/^[1-9][0-9]{0,18}$/.test(value || "")) fail("Invalid customer identifier.");
  return value;
}
function hostID(value: string) {
  if (!/^(?:[a-z][a-z0-9-]{0,62}|[1-9][0-9]{0,18})$/.test(value || "")) fail("Invalid mail host identifier.");
  return value;
}
type ServiceRow = { customer_id: string; desired_enabled: boolean; state: MailService["state"]; revision: string };
function view(id: string, row?: ServiceRow): MailService {
  return row ? { customerID: id, enabled: row.desired_enabled, state: row.state, revision: String(row.revision) }
    : { customerID: id, enabled: false, state: "disabled", revision: "0" };
}
async function transaction<T>(pool: Pool, operation: (client: PoolClient) => Promise<T>): Promise<T> {
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    await client.query("SET LOCAL lock_timeout='5s'");
    const result = await operation(client);
    await client.query("COMMIT");
    return result;
  } catch (error) { await client.query("ROLLBACK"); throw error; }
  finally { client.release(); }
}

/** The caller must authorize this customer's current membership before reading. */
export async function getMailService(pool: Pool, id: string): Promise<MailService> {
  return view(customerID(id), (await pool.query("SELECT customer_id,desired_enabled,state,revision FROM webdock_mail.service WHERE customer_id=$1", [id])).rows[0]);
}

/** Explicit incoming Mail activation only; outgoing-only never calls this provisioner. */
export async function requestMailService(pool: Pool, input: { customerID: string; enabled: boolean; actorID: string; expectedRevision: string; hostID: string; reconcile?: boolean }, identity?: Pick<IdentityDirectory, "subject">): Promise<MailService> {
  const id = customerID(input.customerID), host = hostID(input.hostID);
  if ((input.reconcile !== undefined && typeof input.reconcile !== "boolean") || typeof input.enabled !== "boolean" || !/^(0|[1-9][0-9]{0,18})$/.test(input.expectedRevision || "")) fail("Invalid mail activation request.");
  return transaction(pool, async client => {
    // Lock authorization and ownership before changing desired state; revocation cannot race this write.
    if (identity) {
      const actor = await identity.subject(client, input.actorID, { lock: true });
      if (!activeIdentity(actor) || actor.subject !== input.actorID || actor.role !== "operator")
        fail("Current platform operator access is required.");
    } else {
      const actor = await client.query(`SELECT id FROM webdock_auth."user" WHERE id=$1 AND role='operator'
        AND NOT coalesce(banned,false) AND "emailVerified" AND "twoFactorEnabled" AND NOT "mustChangePassword" FOR SHARE`, [input.actorID]);
      if (!actor.rowCount) fail("Current platform operator access is required.");
    }
    const customer = (await client.query(`SELECT c.status FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id
      WHERE c.id=$1 FOR UPDATE OF c`, [id])).rows[0];
    if (!customer) fail("Customer not found.");
    if (input.enabled) {
      if (customer.status !== "active") fail("An active customer is required.");
      const settings = (await client.query("SELECT settings FROM webdock_auth.platform_settings WHERE id=true FOR SHARE")).rows[0];
      if (settings?.settings?.mailEnabled !== true) fail("Mail is disabled by the platform operator.");
    }
    const current = (await client.query("SELECT * FROM webdock_mail.service WHERE customer_id=$1 FOR UPDATE", [id])).rows[0];
    if (input.reconcile) {
      if (!current || current.state !== "needs_review" || current.desired_enabled !== input.enabled)
        fail("Only an operation awaiting review can be retried.");
      const verified = (await client.query("SELECT customer_id FROM webdock_mail.instance WHERE customer_id=$1 AND verified_at IS NOT NULL", [id])).rowCount;
      if (!verified) fail("Only previously verified Mail instances can be retried.");
      if ((await client.query("SELECT id FROM webdock_mail.operation WHERE customer_id=$1 AND state='running'", [id])).rowCount)
        fail("The previous Mail operation is still running.");
    }
    if (!input.reconcile && (current?.desired_enabled === input.enabled || (!current && !input.enabled))) return view(id, current);
    if (String(current?.revision ?? "0") !== input.expectedRevision) fail("Mail settings changed. Refresh before trying again.");
    // Incoming activation changes the sending path. The customer lock fences concurrent admissions.
    // The budget table is additive and may be absent on foundation-only installations.
    if (!current && input.enabled && (await client.query("SELECT to_regclass('webdock_mail.budget_reservation') AS present")).rows[0].present) {
      if ((await client.query("SELECT reference_id FROM webdock_mail.budget_reservation WHERE customer_id=$1 AND state='reserved' LIMIT 1", [id])).rowCount)
        fail("Reconcile pending sends before activating incoming Mail.");
    }
    if (!current && input.enabled && (await client.query("SELECT to_regclass('webdock_auth.mail_message') AS present")).rows[0].present) {
      if ((await client.query("SELECT id FROM webdock_auth.mail_message WHERE customer_id=$1 AND NOT simulated AND dispatch_status IN ('scheduled','dispatching','outcome_unknown') LIMIT 1",[id])).rowCount)
        fail("Reconcile pending sends before activating incoming Mail.");
    }
    const revision = current ? String(BigInt(current.revision) + 1n) : "1";
    const result = await client.query(`INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,desired_enabled,state,revision)
      VALUES($1,$2,$3,$4,'pending',$5) ON CONFLICT(customer_id) DO UPDATE SET desired_enabled=$4,
      state=CASE WHEN service.state='needs_review' AND NOT $6 THEN 'needs_review' ELSE 'pending' END,revision=$5,updated_at=now() RETURNING *`,
    [id, `mail-${id}`, host, input.enabled, revision, input.reconcile === true]);
    const targetHost = result.rows[0].host_id;
    if (input.enabled && /^[1-9][0-9]*$/.test(targetHost)) {
      try { await reserveMailCapacity(client, id, targetHost); }
      catch (error) { if (error instanceof MailCapacityError) fail(error.message); throw error; }
    }
    await client.query("UPDATE webdock_mail.operation SET state='superseded',finished_at=coalesce(finished_at,now()) WHERE customer_id=$1 AND (state='pending' OR ($2 AND state='needs_review'))", [id, input.reconcile === true]);
    await client.query(`INSERT INTO webdock_mail.operation(customer_id,revision,desired_enabled,requested_by) VALUES($1,$2,$3,$4)`,
      [id, revision, input.enabled, input.actorID]);
    return view(id, result.rows[0]);
  });
}

export async function claimMailOperation(pool: Pool, host: string): Promise<MailOperation | null> {
  hostID(host);
  return transaction(pool, async client => {
    // An expired lease can hide a completed external write. Never re-issue it automatically.
    await client.query(`WITH locked AS (
      SELECT s.customer_id FROM webdock_mail.service s WHERE s.host_id=$1 AND EXISTS (
        SELECT 1 FROM webdock_mail.operation o WHERE o.customer_id=s.customer_id AND o.state='running' AND o.lease_until<now()
      ) FOR UPDATE SKIP LOCKED
    ), expired AS (
      UPDATE webdock_mail.operation SET state='needs_review',finished_at=now()
      WHERE customer_id IN (SELECT customer_id FROM locked) AND state='running' AND lease_until<now() RETURNING customer_id
    ) UPDATE webdock_mail.service SET state='needs_review',updated_at=now() WHERE customer_id IN (SELECT customer_id FROM expired)`, [host]);
    const operation = (await client.query(`SELECT o.*,s.instance_key,s.host_id FROM webdock_mail.service s
      JOIN webdock_mail.operation o ON o.customer_id=s.customer_id AND o.revision=s.revision
      JOIN webdock_admin.customers c ON c.id=s.customer_id
      WHERE s.host_id=$1 AND o.state='pending' AND s.state<>'needs_review'
        AND (NOT o.desired_enabled OR (c.status='active' AND EXISTS (
          SELECT 1 FROM webdock_auth.platform_settings p WHERE p.id=true AND p.settings->>'mailEnabled'='true')))
        AND NOT EXISTS (SELECT 1 FROM webdock_mail.operation previous WHERE previous.customer_id=s.customer_id AND previous.state IN ('running','needs_review'))
      ORDER BY o.created_at,o.id FOR UPDATE OF s,o SKIP LOCKED LIMIT 1`, [host])).rows[0];
    if (!operation) return null;
    const token = randomUUID();
    const leased = (await client.query("UPDATE webdock_mail.operation SET state='running',lease_token=$2,lease_until=now()+interval '2 minutes' WHERE id=$1 RETURNING lease_until", [operation.id, token])).rows[0];
    await client.query("UPDATE webdock_mail.service SET state='provisioning',updated_at=now() WHERE customer_id=$1", [operation.customer_id]);
    return { id: operation.id, customerID: operation.customer_id, revision: String(operation.revision), enabled: operation.desired_enabled,
      instanceKey: operation.instance_key, hostID: operation.host_id, leaseToken: token, leaseUntil: new Date(leased.lease_until).toISOString() };
  });
}

export async function finishMailOperation(pool: Pool, operation: MailOperation, outcome: "succeeded" | "needs_review"): Promise<boolean> {
  if (!["succeeded", "needs_review"].includes(outcome)) fail("Invalid operation outcome.");
  return transaction(pool, client => finishMailOperationInTransaction(client, operation, outcome));
}

/** Caller holds a transaction; allows agent fencing and credential updates to commit atomically. */
export async function finishMailOperationInTransaction(client: Pick<PoolClient, "query">, operation: MailOperation, outcome: "succeeded" | "needs_review"): Promise<boolean> {
    if (!["succeeded", "needs_review"].includes(outcome)) fail("Invalid operation outcome.");
    const service = (await client.query("SELECT * FROM webdock_mail.service WHERE customer_id=$1 FOR UPDATE", [customerID(operation.customerID)])).rows[0];
    if (!service) return false;
    const updated = await client.query(`UPDATE webdock_mail.operation SET state=$4,finished_at=now()
      WHERE id=$1 AND customer_id=$2 AND lease_token=$3 AND state='running' AND lease_until>=now() RETURNING *`,
    [operation.id, operation.customerID, operation.leaseToken, outcome]);
    const completed = updated.rows[0];
    if (!completed) return false;
    if (outcome === "succeeded" && String(service.revision) === String(completed.revision) && /^[1-9][0-9]*$/.test(service.host_id))
      await completeMailCapacity(client, operation.customerID, completed.desired_enabled);
    const state = outcome === "needs_review" ? "needs_review" : String(service.revision) !== String(completed.revision)
      ? "pending" : completed.desired_enabled ? "ready" : "suspended";
    await client.query(`UPDATE webdock_mail.service SET state=$2,
      applied_revision=CASE WHEN $3='succeeded' THEN $4 ELSE applied_revision END,updated_at=now() WHERE customer_id=$1`,
    [operation.customerID, state, outcome, completed.revision]);
    return true;
}
