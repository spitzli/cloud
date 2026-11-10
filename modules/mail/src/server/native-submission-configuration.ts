import { createHash, randomBytes } from "node:crypto";
import { z } from "zod";
import { MailServiceError } from "./instances";
import { openSecret, sealSecret } from "./secrets";
import { type Connection } from "./native-ports";
import type { createNativeSendTransport } from "./native-send-transport";
import type { NativeMailPorts } from "./native-ports";
const provider = z.string().regex(/^[A-Za-z0-9_-]{1,64}$/);

export const submissionResources = z
  .object({
    accountID: provider,
    hookID: provider,
    routeID: provider,
    tlsID: provider,
    certificateID: provider,
  })
  .strict();

export const submissionConfigurationProof = z.discriminatedUnion("ready", [
  z.object({ ready: z.literal(false) }).strict(),
  z
    .object({
      ready: z.literal(true),
      resources: submissionResources,
      certificateSha256: z.string().regex(/^[a-f0-9]{64}$/),
    })
    .strict(),
]);

export type SubmissionDesired = {
  transportRevision: string;
  resourceIDs: z.infer<typeof submissionResources> | null;
};
export interface NativeSubmissionConfigurationDependencies {
  nativeRelay: ReturnType<typeof createNativeSendTransport>["nativeRelay"];
  env: NativeMailPorts["env"];
}
export function createNativeSubmissionConfiguration(
  dependencies: NativeSubmissionConfigurationDependencies,
) {
  const { nativeRelay, env } = dependencies;
  async function prepareSubmissionConfiguration(
    db: Connection,
    customerID: string,
    hostname: string,
    serviceRevision: string,
    force = false,
  ) {
    if (env.SPITZLI_NATIVE_MAIL_SENDING_ENABLED !== "true") return null;
    let connection: Awaited<ReturnType<typeof nativeRelay>>;
    try {
      connection = await nativeRelay(db, customerID);
    } catch (error) {
      if (error instanceof MailServiceError) return null;
      throw error;
    }
    const instance = (
      await db.query(
        "SELECT * FROM webdock_mail.instance WHERE customer_id=$1 FOR UPDATE",
        [customerID],
      )
    ).rows[0];
    if (!instance) throw Error("Mail instance is unavailable.");
    if (
      !force &&
      instance.submission_state === "ready" &&
      String(instance.submission_ready_revision) === serviceRevision &&
      String(instance.submission_transport_revision) ===
        connection.transportRevision
    )
      return null;
    const purpose = `submission:${customerID}`,
      key = env.MAIL_ENCRYPTION_KEY || "";
    const secret = instance.encrypted_submission_credentials
      ? z
          .object({
            username: z.string(),
            password: z.string().regex(/^[A-Za-z0-9_-]{32,128}$/),
            hookToken: z.string(),
          })
          .strict()
          .parse(
            openSecret(instance.encrypted_submission_credentials, purpose, key),
          )
      : {
          username: `wd-submit@${hostname}`,
          password: randomBytes(48).toString("base64url"),
          hookToken: `wd_hook_${customerID}_${randomBytes(32).toString("base64url")}`,
        };
    if (
      secret.username !== `wd-submit@${hostname}` ||
      !new RegExp(`^wd_hook_${customerID}_[A-Za-z0-9_-]{43}$`).test(
        secret.hookToken,
      )
    )
      throw Error("Mail submission identity changed.");
    const desired: SubmissionDesired = {
      transportRevision: connection.transportRevision,
      resourceIDs: instance.submission_resource_ids
        ? submissionResources.parse(instance.submission_resource_ids)
        : null,
    };
    await db.query(
      "UPDATE webdock_mail.instance SET encrypted_submission_credentials=$2,submission_hook_token_hash=$3,submission_state='pending',submission_ready_revision=NULL WHERE customer_id=$1",
      [
        customerID,
        instance.encrypted_submission_credentials ??
          sealSecret(secret, purpose, key),
        createHash("sha256").update(secret.hookToken).digest("hex"),
      ],
    );
    return {
      desired,
      profile: {
        customerID,
        hostname,
        ...secret,
        relayHost:
          connection.region === "eu"
            ? "pro.eu.turbo-smtp.com"
            : "pro.turbo-smtp.com",
        relayUsername: connection.credentials.consumerKey,
        relayPassword: connection.credentials.consumerSecret,
        ...desired,
      },
    };
  }

  async function completeSubmissionConfiguration(
    db: Connection,
    customerID: string,
    serviceRevision: string,
    desired: SubmissionDesired,
    proof: z.infer<typeof submissionConfigurationProof>,
  ) {
    let current: Awaited<ReturnType<typeof nativeRelay>> | null = null;
    try {
      current = await nativeRelay(db, customerID);
    } catch (error) {
      if (!(error instanceof MailServiceError)) throw error;
    }
    if (
      !proof.ready ||
      current?.transportRevision !== desired.transportRevision
    ) {
      await db.query(
        "UPDATE webdock_mail.instance SET submission_state='needs_review',submission_ready_revision=NULL WHERE customer_id=$1",
        [customerID],
      );
      return;
    }
    if (
      desired.resourceIDs &&
      Object.entries(desired.resourceIDs).some(
        ([name, value]) =>
          proof.resources[name as keyof typeof proof.resources] !== value,
      )
    )
      throw Error("Submission resource ownership changed.");
    await db.query(
      `UPDATE webdock_mail.instance SET submission_state='ready',submission_ready_revision=$2,submission_transport_revision=$3,
 submission_resource_ids=$4,smtp_certificate_sha256=$5,smtp_certificate_revision=$2 WHERE customer_id=$1`,
      [
        customerID,
        serviceRevision,
        desired.transportRevision,
        JSON.stringify(proof.resources),
        proof.certificateSha256,
      ],
    );
  }

  /** A restart changes the service revision, so submission requires a fresh runtime proof. */
  async function reconcileSubmissionConfiguration(
    db: Connection,
    clusterID: string,
  ) {
    if (env.SPITZLI_NATIVE_MAIL_SENDING_ENABLED !== "true") return;
    const row = (
      await db.query(
        `SELECT s.customer_id FROM webdock_mail.service s JOIN webdock_mail.instance i USING(customer_id) JOIN webdock_mail.send_transport t USING(customer_id)
 WHERE EXISTS(SELECT 1 FROM webdock_auth.platform_settings WHERE id=true AND settings->>'mailEnabled'='true')
 AND EXISTS(SELECT 1 FROM webdock_admin.customers WHERE id=s.customer_id AND status='active')
 AND s.host_id=$1 AND s.desired_enabled AND s.state='ready' AND s.revision=s.applied_revision AND t.state='ready'
 AND coalesce(i.submission_state,'pending')<>'needs_review'
 AND (i.submission_ready_revision IS DISTINCT FROM s.revision OR i.submission_transport_revision IS DISTINCT FROM t.revision)
 AND NOT EXISTS(SELECT 1 FROM webdock_mail.configuration_operation o WHERE o.customer_id=s.customer_id AND o.state IN ('pending','running','needs_review'))
 ORDER BY s.customer_id FOR UPDATE OF s SKIP LOCKED LIMIT 1`,
        [clusterID],
      )
    ).rows[0];
    if (!row) return;
    const locked = (
      await db.query(
        "SELECT pg_try_advisory_xact_lock(hashtextextended($1,0)) AS locked",
        ["mail-configuration:" + row.customer_id],
      )
    ).rows[0].locked;
    if (!locked) return;
    const version = (
      await db.query(
        `INSERT INTO webdock_mail.configuration(customer_id) VALUES($1) ON CONFLICT(customer_id) DO UPDATE SET revision=configuration.revision+1 RETURNING revision`,
        [row.customer_id],
      )
    ).rows[0].revision;
    await db.query(
      "INSERT INTO webdock_mail.configuration_operation(customer_id,revision,reason) VALUES($1,$2,'submission_reconcile')",
      [row.customer_id, version],
    );
  }
  return {
    prepareSubmissionConfiguration,
    completeSubmissionConfiguration,
    reconcileSubmissionConfiguration,
  };
}
