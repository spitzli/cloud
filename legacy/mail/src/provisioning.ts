import type { Pool } from "pg";
import { randomBytes } from "node:crypto";
import { claimMailOperation, finishMailOperation } from "@webdock/mail-core";
import { sealSecret, openSecret } from "@webdock/mail-core/secrets";
import type { DockerMailRuntime, InstanceCredentials } from "./runtime/docker.ts";

export async function processMailOperation(pool: Pool, runtime: Pick<DockerMailRuntime, "provision" | "suspend">, hostID: string, key: string): Promise<boolean> {
  // Validate host secret configuration before claiming anything that could need external reconciliation.
  sealSecret(null, "worker-configuration", key);
  const operation = await claimMailOperation(pool, hostID);
  if (!operation) return false;
  const heartbeat = setInterval(() => {
    void pool.query(`UPDATE webdock_mail.operation SET lease_until=now()+interval '2 minutes'
      WHERE id=$1 AND lease_token=$2 AND state='running' AND lease_until>=now()`, [operation.id, operation.leaseToken]).catch(() => {});
  }, 20_000);
  heartbeat.unref();
  try {
    if (!operation.enabled) {
      await runtime.suspend(operation);
    } else {
      const purpose = `instance:${operation.customerID}`;
      const initial = sealSecret({ bootstrapPassword: randomBytes(32).toString("base64url") }, purpose, key);
      await pool.query(`INSERT INTO webdock_mail.instance(customer_id,encrypted_credentials) VALUES($1,$2) ON CONFLICT(customer_id) DO NOTHING`, [operation.customerID, initial]);
      const stored = (await pool.query("SELECT encrypted_credentials FROM webdock_mail.instance WHERE customer_id=$1", [operation.customerID])).rows[0];
      const credentials = openSecret<InstanceCredentials>(stored.encrypted_credentials, purpose, key);
      const address = await runtime.provision(operation, credentials, async next => {
        const saved = await pool.query(`UPDATE webdock_mail.instance SET encrypted_credentials=$2,updated_at=now() WHERE customer_id=$1
          AND EXISTS (SELECT 1 FROM webdock_mail.operation WHERE id=$3 AND lease_token=$4 AND state='running' AND lease_until>=now())`,
        [operation.customerID, sealSecret(next, purpose, key), operation.id, operation.leaseToken]);
        if (!saved.rowCount) throw new Error("Mail provisioning lease lost; reconcile existing resources");
      });
      await pool.query(`UPDATE webdock_mail.instance SET internal_url=$2,public_url=$3,verified_at=now(),updated_at=now() WHERE customer_id=$1
        AND EXISTS (SELECT 1 FROM webdock_mail.operation WHERE id=$4 AND lease_token=$5 AND state='running' AND lease_until>=now())`,
      [operation.customerID, address.internalURL, address.publicURL, operation.id, operation.leaseToken]);
    }
    await finishMailOperation(pool, operation, "succeeded");
  } catch {
    // Infrastructure errors can contain credentials. Persist only the safe review state, never raw messages.
    await finishMailOperation(pool, operation, "needs_review");
  } finally { clearInterval(heartbeat); }
  return true;
}
