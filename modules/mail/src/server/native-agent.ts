import { randomBytes } from "node:crypto";
import { z } from "zod";
import { HostingError, resourceID } from "@spitzli/hosting-contracts";
import {
  claimMailOperation,
  finishMailOperationInTransaction,
  type MailOperation,
} from "./instances";
import { mailInstanceDemand } from "./capacity";
import { openSecret, sealSecret } from "./secrets";
import { type Agent } from "./native-ports";
import { type Connection } from "./native-ports";
import type { NativeMailPorts } from "./native-ports";
type Credentials = {
  bootstrapPassword?: string;
  username?: string;
  password?: string;
};

const credential = z
  .string()
  .min(1)
  .max(1024)
  .regex(/^[^\x00-\x1f]+$/);

const lease = z.object({
  operationID: resourceID,
  leaseToken: z.string().uuid(),
  generation: z.number().int().positive(),
});

const checkpoint = lease
  .extend({
    credentials: z
      .object({ username: credential, password: credential })
      .strict()
      .optional(),
  })
  .strict();

const completion = lease
  .extend({
    outcome: z.enum(["succeeded", "needs_review"]),
    proof: z
      .object({
        namespace: z.string(),
        revision: z.number().int().positive(),
        running: z.boolean(),
        edition: z.literal("community"),
        recoveryDisabled: z.literal(true),
        submissionCertificateSha256: z
          .string()
          .regex(/^[a-f0-9]{64}$/)
          .optional(),
      })
      .strict()
      .optional(),
  })
  .strict();
export interface NativeAgentDependencies {
  database: NativeMailPorts["database"];
  verifyAgent: NativeMailPorts["verifyAgent"];
  transaction: NativeMailPorts["transaction"];
  env: NativeMailPorts["env"];
}
export function createNativeAgent(dependencies: NativeAgentDependencies) {
  const { database, verifyAgent, transaction, env } = dependencies;
  function conflict(): never {
    throw new HostingError(
      409,
      "Mail operation is no longer available to this agent.",
    );
  }

  function mailAgentConfig() {
    const key = env.MAIL_ENCRYPTION_KEY || "",
      image = env.MAIL_STALWART_IMAGE || "",
      suffix = env.MAIL_HOSTNAME_SUFFIX || "";
    if (
      !/^[A-Za-z0-9.:-]+\/[A-Za-z0-9_./-]+@sha256:[a-f0-9]{64}$/.test(image) ||
      suffix.length > 220 ||
      !/^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/.test(suffix)
    )
      throw new HostingError(503, "Managed Mail is not configured.");
    sealSecret({}, "configuration-check", key);
    return { key, image, suffix };
  }

  async function verifyMailAgent(db: Connection, agent: Agent) {
    const observed = await verifyAgent(db, agent);
    const c = (
      await db.query(
        "SELECT ownership,provider,verified FROM webdock_auth.hosting_cluster WHERE id=$1 FOR SHARE",
        [agent.clusterID],
      )
    ).rows[0];
    if (
      !c ||
      c.ownership !== "platform" ||
      !/^v[0-9][^,]*\+k3s[0-9]+$/.test(observed.observation?.version || "") ||
      !c.verified
    )
      conflict();
    return observed;
  }

  async function operation(
    db: Connection,
    agent: Agent,
    input: z.infer<typeof lease>,
  ): Promise<MailOperation> {
    await verifyMailAgent(db, agent);
    if (input.generation !== agent.generation) conflict();
    // Match the core lock order: service before operation.
    const s = (
      await db.query(
        `SELECT s.* FROM webdock_mail.service s JOIN webdock_mail.operation o ON o.customer_id=s.customer_id
    WHERE o.id=$1 AND s.host_id=$2 FOR UPDATE OF s`,
        [input.operationID, agent.clusterID],
      )
    ).rows[0];
    if (!s) conflict();
    const o = (
      await db.query(
        `SELECT * FROM webdock_mail.operation WHERE id=$1 AND lease_token=$2
    AND state='running' AND lease_until>=now() AND agent_generation=$3 FOR UPDATE`,
        [input.operationID, input.leaseToken, agent.generation],
      )
    ).rows[0];
    if (!o) conflict();
    return {
      id: o.id,
      customerID: o.customer_id,
      revision: String(o.revision),
      enabled: o.desired_enabled,
      instanceKey: s.instance_key,
      hostID: s.host_id,
      leaseToken: input.leaseToken,
      leaseUntil: new Date(o.lease_until).toISOString(),
    };
  }

  async function readCredentials(
    db: Connection,
    id: string,
    key: string,
  ): Promise<Credentials> {
    const row = (
      await db.query(
        "SELECT encrypted_credentials FROM webdock_mail.instance WHERE customer_id=$1 FOR UPDATE",
        [id],
      )
    ).rows[0];
    return row
      ? openSecret<Credentials>(
          row.encrypted_credentials,
          `instance:${id}`,
          key,
        )
      : {};
  }

  async function saveCredentials(
    db: Connection,
    id: string,
    value: Credentials,
    key: string,
  ) {
    await db.query(
      `INSERT INTO webdock_mail.instance(customer_id,encrypted_credentials) VALUES($1,$2)
    ON CONFLICT(customer_id) DO UPDATE SET encrypted_credentials=excluded.encrypted_credentials,updated_at=now()`,
      [id, sealSecret(value, `instance:${id}`, key)],
    );
  }

  async function claimNativeMail(agent: Agent) {
    if (env.SPITZLI_NATIVE_MAIL_ENABLED !== "true") return null;
    const settings = mailAgentConfig();
    await transaction(async (db) => {
      const a = await verifyMailAgent(db, agent);
      if (
        a.observation?.capabilities?.nativeMail?.version !== 1 ||
        a.observation?.capabilities?.nativeMail?.image !== settings.image ||
        a.observation?.capabilities?.storageVersion !== 1
      )
        conflict();
    });
    const job = await claimMailOperation(database, agent.clusterID);
    if (!job) return null;
    return transaction(async (db) => {
      await verifyMailAgent(db, agent);
      await db.query(
        "SELECT customer_id FROM webdock_mail.service WHERE customer_id=$1 FOR UPDATE",
        [job.customerID],
      );
      await db.query(
        "UPDATE webdock_mail.operation SET agent_generation=$3 WHERE id=$1 AND lease_token=$2 AND agent_generation IS NULL AND state='running'",
        [job.id, job.leaseToken, agent.generation],
      );
      await operation(db, agent, {
        operationID: job.id,
        leaseToken: job.leaseToken,
        generation: agent.generation,
      });
      const revision = Number(job.revision);
      if (!Number.isSafeInteger(revision) || revision > 2147483647) conflict();
      const credentials = await readCredentials(
        db,
        job.customerID,
        settings.key,
      );
      if (
        job.enabled &&
        !credentials.bootstrapPassword &&
        !(credentials.username && credentials.password)
      ) {
        credentials.bootstrapPassword = randomBytes(32).toString("base64url");
        await saveCredentials(db, job.customerID, credentials, settings.key);
      }
      const { cpuMillicores, memoryBytes, volumeBytes, ephemeralBytes } =
        mailInstanceDemand;
      return {
        version: 1,
        customerID: job.customerID,
        clusterID: agent.clusterID,
        operationID: job.id,
        revision,
        generation: agent.generation,
        leaseToken: job.leaseToken,
        leaseUntil: job.leaseUntil,
        action: job.enabled ? "ensure" : "suspend",
        hostname: `mail-${job.customerID}.${settings.suffix}`,
        image: settings.image,
        resources: { cpuMillicores, memoryBytes, volumeBytes, ephemeralBytes },
        credentials,
      };
    });
  }

  async function checkpointNativeMail(agent: Agent, input: unknown) {
    const parsed = checkpoint.parse(input);
    return transaction(async (db) => {
      const job = await operation(db, agent, parsed);
      if (parsed.credentials) {
        if (!job.enabled) conflict();
        const { key } = mailAgentConfig(),
          previous = await readCredentials(db, job.customerID, key);
        // Retransmission is safe; replacing persisted permanent credentials is not.
        if (
          previous.username &&
          (previous.username !== parsed.credentials.username ||
            previous.password !== parsed.credentials.password)
        )
          conflict();
        await saveCredentials(
          db,
          job.customerID,
          { ...previous, ...parsed.credentials },
          key,
        );
      }
      const row = (
        await db.query(
          "UPDATE webdock_mail.operation SET lease_until=now()+interval '2 minutes' WHERE id=$1 RETURNING lease_until",
          [job.id],
        )
      ).rows[0];
      return { leaseUntil: new Date(row.lease_until).toISOString() };
    });
  }

  async function completeNativeMail(agent: Agent, input: unknown) {
    const parsed = completion.parse(input);
    return transaction(async (db) => {
      const job = await operation(db, agent, parsed);
      if (parsed.outcome === "succeeded") {
        const p = parsed.proof;
        if (
          !p ||
          p.namespace !== `wd-mail-${job.customerID}` ||
          String(p.revision) !== job.revision ||
          p.running !== job.enabled
        )
          conflict();
        if (job.enabled) {
          const { key, suffix } = mailAgentConfig(),
            value = await readCredentials(db, job.customerID, key);
          if (!value.username || !value.password) conflict();
          await saveCredentials(
            db,
            job.customerID,
            { username: value.username, password: value.password },
            key,
          );
          await db.query(
            "UPDATE webdock_mail.instance SET internal_url=$2,public_url=$3,verified_at=now() WHERE customer_id=$1",
            [
              job.customerID,
              `http://app-${job.customerID}.wd-mail-${job.customerID}.svc.cluster.local:8080`,
              `https://mail-${job.customerID}.${suffix}`,
            ],
          );
          // Old agents remain compatible, but their completions cannot enable native SMTP sending.
          await db.query(
            "UPDATE webdock_mail.instance SET smtp_certificate_sha256=$2,smtp_certificate_revision=$3 WHERE customer_id=$1",
            [
              job.customerID,
              p.submissionCertificateSha256 ?? null,
              p.submissionCertificateSha256 ? job.revision : null,
            ],
          );
        }
      }
      if (!(await finishMailOperationInTransaction(db, job, parsed.outcome)))
        conflict();
      return { completed: true };
    });
  }
  return {
    mailAgentConfig,
    verifyMailAgent,
    claimNativeMail,
    checkpointNativeMail,
    completeNativeMail,
  };
}
