import {
  submissionConfigurationProof,
  type SubmissionDesired,
} from "./native-submission-configuration";
import { createPublicKey, randomBytes, randomUUID } from "node:crypto";
import { z } from "zod";
import { HostingError, resourceID } from "@spitzli/hosting-contracts";
import { openSecret, sealSecret } from "./secrets";
import { type Agent } from "./native-ports";
import { type Connection } from "./native-ports";
import type { createNativeSubmissionConfiguration } from "./native-submission-configuration";
import type { createNativeAgent } from "./native-agent";
import type { NativeMailPorts } from "./native-ports";
const leaseSchema = z
  .object({
    operationID: resourceID,
    leaseToken: z.string().uuid(),
    generation: z.number().int().positive(),
  })
  .strict();

const providerID = z.string().regex(/^[A-Za-z0-9_-]{1,64}$/);

const dkimKey = z
  .object({
    id: providerID,
    selector: z.string().regex(/^(?:webdock-rsa-|v1-rsa-)[a-z0-9-]{1,40}$/),
    publicKey: z
      .string()
      .min(128)
      .max(2048)
      .regex(/^[A-Za-z0-9+/]+={0,2}$/)
      .refine((value) => {
        try {
          const key = createPublicKey({
            key: Buffer.from(value, "base64"),
            format: "der",
            type: "spki",
          });
          return (
            key.asymmetricKeyType === "rsa" &&
            (key.asymmetricKeyDetails?.modulusLength ?? 0) >= 2048
          );
        } catch {
          return false;
        }
      }),
  })
  .strict();

const completionSchema = leaseSchema
  .extend({
    outcome: z.enum(["succeeded", "needs_review"]),
    proof: z
      .object({
        domains: z.record(resourceID, providerID),
        mailboxes: z.record(resourceID, providerID),
        identities: z.record(resourceID, providerID),
        dkim: z.record(resourceID, z.array(dkimKey).min(1).max(2)),
        submission: submissionConfigurationProof.nullable().optional(),
      })
      .strict()
      .optional(),
  })
  .strict();

type Domain = {
  id: string;
  name: string;
  providerId: string | null;
  ownershipVerified: boolean;
};

type Mailbox = {
  id: string;
  domainId: string;
  localPart: string;
  quotaBytes: number;
  providerId: string | null;
};

type Identity = {
  id: string;
  subject: string;
  providerId: string | null;
  enabled: boolean;
  mailboxIds: string[];
};

type Desired = {
  submission?: SubmissionDesired | null;
  serviceRevision: string;
  domains: Domain[];
  mailboxes: Mailbox[];
  identities: Identity[];
  grants: { mailboxID: string; subject: string; revision: string }[];
};
export interface NativeConfigurationDependencies {
  clerkMailAccess?: NativeMailPorts["clerkMailAccess"];

  prepareSubmissionConfiguration: ReturnType<
    typeof createNativeSubmissionConfiguration
  >["prepareSubmissionConfiguration"];
  completeSubmissionConfiguration: ReturnType<
    typeof createNativeSubmissionConfiguration
  >["completeSubmissionConfiguration"];
  reconcileSubmissionConfiguration: ReturnType<
    typeof createNativeSubmissionConfiguration
  >["reconcileSubmissionConfiguration"];
  mailAgentConfig: ReturnType<typeof createNativeAgent>["mailAgentConfig"];
  verifyMailAgent: ReturnType<typeof createNativeAgent>["verifyMailAgent"];
  transaction: NativeMailPorts["transaction"];
  env: NativeMailPorts["env"];
}
export function createNativeConfiguration(
  dependencies: NativeConfigurationDependencies,
) {
  const {
    clerkMailAccess = async () => undefined,
    prepareSubmissionConfiguration,
    completeSubmissionConfiguration,
    reconcileSubmissionConfiguration,
    mailAgentConfig,
    verifyMailAgent,
    transaction,
    env,
  } = dependencies;
  function unavailable(): never {
    throw new HostingError(409, "Mail configuration operation is unavailable.");
  }

  async function lock(
    db: Connection,
    agent: Agent,
    input: z.infer<typeof leaseSchema>,
  ) {
    await verifyMailAgent(db, agent);
    if (input.generation !== agent.generation) unavailable();
    const candidate = (
      await db.query(
        `SELECT o.customer_id FROM webdock_mail.configuration_operation o JOIN webdock_mail.service s USING(customer_id)
    WHERE o.id=$1 AND s.host_id=$2`,
        [input.operationID, agent.clusterID],
      )
    ).rows[0];
    if (!candidate) unavailable();
    await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
      "mail-configuration:" + candidate.customer_id,
    ]);
    const row = (
      await db.query(
        `SELECT * FROM webdock_mail.configuration_operation WHERE id=$1 AND state='running' AND lease_token=$2 AND agent_generation=$3 AND lease_until>=now() FOR UPDATE`,
        [input.operationID, input.leaseToken, agent.generation],
      )
    ).rows[0];
    if (!row?.desired) unavailable();
    return row as {
      id: string;
      customer_id: string;
      revision: string;
      desired: Desired;
    };
  }

  async function claimMailConfiguration(agent: Agent) {
    if (env.WEBDOCK_NATIVE_MAILBOXES_ENABLED !== "true") return null;
    const settings = mailAgentConfig();
    return transaction(async (db) => {
      const active = await verifyMailAgent(db, agent);
      if (
        active.observation?.capabilities?.nativeMail?.configurationVersion !== 3
      )
        return null;
      await reconcileSubmissionConfiguration(db, agent.clusterID);
      await db.query(
        `UPDATE webdock_mail.configuration_operation o SET state='needs_review',finished_at=now()
      FROM webdock_mail.service s WHERE s.customer_id=o.customer_id AND s.host_id=$1 AND o.state='running' AND o.lease_until<now()`,
        [agent.clusterID],
      );
      const candidate = (
        await db.query(
          `SELECT o.customer_id,o.id,o.reason FROM webdock_mail.configuration_operation o
      JOIN webdock_mail.configuration c USING(customer_id) JOIN webdock_mail.service s USING(customer_id)
      WHERE s.host_id=$1 AND s.desired_enabled AND s.state='ready' AND s.revision=s.applied_revision AND o.revision=c.revision AND o.state='pending'
      AND NOT EXISTS(SELECT 1 FROM webdock_mail.configuration_operation p WHERE p.customer_id=o.customer_id AND p.state IN ('running','needs_review'))
      ORDER BY o.created_at,o.id LIMIT 1`,
          [agent.clusterID],
        )
      ).rows[0];
      if (!candidate) return null;
      const locked = (
        await db.query(
          "SELECT pg_try_advisory_xact_lock(hashtextextended($1,0)) AS locked",
          ["mail-configuration:" + candidate.customer_id],
        )
      ).rows[0].locked;
      if (!locked) return null;
      const current = (
        await db.query(
          `SELECT s.revision,i.encrypted_credentials FROM webdock_mail.service s JOIN webdock_mail.instance i USING(customer_id)
      JOIN webdock_mail.configuration c USING(customer_id) JOIN webdock_mail.configuration_operation o ON o.customer_id=c.customer_id AND o.revision=c.revision
      WHERE o.id=$1 AND o.state='pending' AND s.desired_enabled AND s.state='ready' AND s.revision=s.applied_revision AND i.verified_at IS NOT NULL
      AND EXISTS(SELECT 1 FROM webdock_admin.customers WHERE id=s.customer_id AND status='active')
      AND EXISTS(SELECT 1 FROM webdock_auth.platform_settings WHERE id=true AND settings->>'mailEnabled'='true') FOR UPDATE OF o FOR SHARE OF s`,
          [candidate.id],
        )
      ).rows[0];
      if (!current) return null;
      const customerID = candidate.customer_id as string;
      const credentials = openSecret<{
        username?: string;
        password?: string;
        bootstrapPassword?: string;
      }>(current.encrypted_credentials, `instance:${customerID}`, settings.key);
      if (
        !credentials.username ||
        !credentials.password ||
        credentials.bootstrapPassword
      )
        unavailable();
      const hostname = `mail-${customerID}.${settings.suffix}`;
      const domains: Domain[] = (
        await db.query(
          `SELECT d.id,d.name,d.provider_id AS "providerId",(s.status='ownership_verified' AND s.deleted_at IS NULL) AS "ownershipVerified" FROM webdock_mail.domain d JOIN webdock_auth.mail_sender_domain s ON s.id=d.sender_domain_id AND s.customer_id=d.customer_id WHERE d.customer_id=$1 ORDER BY d.id`,
          [customerID],
        )
      ).rows;
      const mailboxes: Mailbox[] = (
        await db.query(
          `SELECT id,domain_id AS "domainId",local_part AS "localPart",quota_bytes::float8 AS "quotaBytes",provider_id AS "providerId" FROM webdock_mail.mailbox WHERE customer_id=$1 AND enabled ORDER BY id`,
          [customerID],
        )
      ).rows;
      const rows = (
        await db.query(
          `SELECT i.*,NOT coalesce(u.banned,false) AND u."emailVerified" AND NOT u."mustChangePassword" AND
      ((u.role='operator' AND u."twoFactorEnabled") OR (u.role='user' AND EXISTS(SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId" WHERE m."userId"=u.id AND t.customer_id=i.customer_id))) AS allowed
      FROM webdock_mail.identity i JOIN webdock_auth."user" u ON u.id=i.subject WHERE i.customer_id=$1 ORDER BY i.id FOR UPDATE OF i`,
          [customerID],
        )
      ).rows;
      const grants = (
        await db.query(
          `SELECT g.mailbox_id AS "mailboxID",g.subject,g.revision,g.clerk_grant,g.enabled AS requested_enabled,g.enabled AND
      ((g.operator_grant AND EXISTS(SELECT 1 FROM webdock_auth."user" u WHERE u.id=g.subject AND u.role='operator' AND u."twoFactorEnabled")) OR EXISTS(SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId" WHERE m.id=g.member_id AND m."userId"=g.subject AND t.customer_id=g.customer_id)) AS enabled
      FROM webdock_mail.mailbox_grant g WHERE g.customer_id=$1 ORDER BY g.mailbox_id,g.subject`,
          [customerID],
        )
      ).rows;
      if (
        domains.length > 32 ||
        mailboxes.length > 64 ||
        rows.length > 128 ||
        grants.filter((g) => g.enabled).length > 256
      )
        unavailable();
      // Provider references are revalidated at dispatch, never inferred from local credentials.
      for (const row of rows) {
        const clerk = await clerkMailAccess(db, row.subject, customerID);
        if (clerk !== undefined) row.allowed = Boolean(clerk);
        for (const grant of grants.filter((g) => g.subject === row.subject)) {
          if (clerk !== undefined || grant.clerk_grant)
            grant.enabled = Boolean(
              grant.requested_enabled &&
              clerk &&
              grant.clerk_grant === clerk.clerkGrant,
            );
        }
      }
      const identities: Identity[] = [],
        secrets = [];
      for (const row of rows) {
        const mailboxIds = row.allowed
          ? grants
              .filter(
                (g) =>
                  g.enabled &&
                  g.subject === row.subject &&
                  mailboxes.some((b) => b.id === g.mailboxID),
              )
              .map((g) => g.mailboxID as string)
          : [];
        const username = `wd-${row.id}@${hostname}`;
        if (!row.encrypted_credentials) {
          if (row.provider_id) unavailable();
          row.encrypted_credentials = sealSecret(
            { username, password: randomBytes(32).toString("base64url") },
            `identity:${customerID}:${row.id}`,
            settings.key,
          );
          await db.query(
            "UPDATE webdock_mail.identity SET encrypted_credentials=$2 WHERE id=$1",
            [row.id, row.encrypted_credentials],
          );
        }
        const secret = openSecret<{ username: string; password: string }>(
          row.encrypted_credentials,
          `identity:${customerID}:${row.id}`,
          settings.key,
        );
        if (secret.username !== username || !secret.password) unavailable();
        const identity: Identity = {
          id: row.id,
          subject: row.subject,
          providerId: row.provider_id,
          enabled: mailboxIds.length > 0,
          mailboxIds,
        };
        identities.push(identity);
        secrets.push({ ...identity, username, password: secret.password });
      }
      const submission = await prepareSubmissionConfiguration(
        db,
        customerID,
        hostname,
        String(current.revision),
        ["submission.configure", "configuration.retry"].includes(
          candidate.reason,
        ),
      );
      const desired: Desired = {
        submission: submission?.desired ?? null,
        serviceRevision: String(current.revision),
        domains,
        mailboxes,
        identities,
        grants: grants.map(({ mailboxID, subject, revision }) => ({
          mailboxID,
          subject,
          revision: String(revision),
        })),
      };
      const token = randomUUID();
      const operation = (
        await db.query(
          "UPDATE webdock_mail.configuration_operation SET state='running',lease_token=$2,lease_until=now()+interval '2 minutes',agent_generation=$3,desired=$4 WHERE id=$1 RETURNING revision,lease_until",
          [candidate.id, token, agent.generation, JSON.stringify(desired)],
        )
      ).rows[0];
      return {
        version: 3,
        customerID,
        clusterID: agent.clusterID,
        operationID: candidate.id as string,
        generation: agent.generation,
        revision: String(operation.revision),
        leaseToken: token,
        leaseUntil: new Date(operation.lease_until).toISOString(),
        hostname,
        credentials: {
          username: credentials.username,
          password: credentials.password,
        },
        domains,
        mailboxes,
        identities: secrets,
        submission: submission?.profile ?? null,
      };
    });
  }

  async function checkpointMailConfiguration(agent: Agent, input: unknown) {
    const parsed = leaseSchema.parse(input);
    return transaction(async (db) => {
      const row = await lock(db, agent, parsed);
      const service = (
        await db.query(
          "SELECT revision,desired_enabled,state FROM webdock_mail.service WHERE customer_id=$1 FOR SHARE",
          [row.customer_id],
        )
      ).rows[0];
      if (
        !service?.desired_enabled ||
        service.state !== "ready" ||
        String(service.revision) !== row.desired.serviceRevision
      )
        unavailable();
      const updated = (
        await db.query(
          "UPDATE webdock_mail.configuration_operation SET lease_until=now()+interval '2 minutes' WHERE id=$1 RETURNING lease_until",
          [row.id],
        )
      ).rows[0];
      return { leaseUntil: new Date(updated.lease_until).toISOString() };
    });
  }

  async function completeMailConfiguration(agent: Agent, input: unknown) {
    const parsed = completionSchema.parse(input);
    return transaction(async (db) => {
      const row = await lock(db, agent, parsed),
        desired = row.desired;
      const service = (
        await db.query(
          "SELECT revision,desired_enabled,state FROM webdock_mail.service WHERE customer_id=$1 FOR SHARE",
          [row.customer_id],
        )
      ).rows[0];
      const succeeded =
        parsed.outcome === "succeeded" &&
        service?.desired_enabled &&
        service.state === "ready" &&
        String(service.revision) === desired.serviceRevision;
      if (succeeded) {
        if (!parsed.proof) unavailable();
        const accountIDs = [
          ...Object.values(parsed.proof.mailboxes),
          ...Object.values(parsed.proof.identities),
          ...(parsed.proof.submission?.ready
            ? [parsed.proof.submission.resources.accountID]
            : []),
        ];
        if (new Set(accountIDs).size !== accountIDs.length) unavailable();
        for (const [kind, table] of [
          ["domains", "domain"],
          ["mailboxes", "mailbox"],
          ["identities", "identity"],
        ] as const) {
          const expected = desired[kind],
            mapping = parsed.proof[kind];
          if (
            Object.keys(mapping).length !== expected.length ||
            new Set(Object.values(mapping)).size !== expected.length
          )
            unavailable();
          for (const entity of expected) {
            const value = mapping[entity.id];
            if (!value || (entity.providerId && entity.providerId !== value))
              unavailable();
            await db.query(
              `UPDATE webdock_mail.${table} SET provider_id=$3,state='ready' WHERE customer_id=$1 AND id=$2`,
              [row.customer_id, entity.id, value],
            );
          }
        }
        for (const grant of desired.grants)
          await db.query(
            "UPDATE webdock_mail.mailbox_grant SET applied_revision=$4 WHERE customer_id=$1 AND mailbox_id=$2 AND subject=$3 AND revision=$4",
            [row.customer_id, grant.mailboxID, grant.subject, grant.revision],
          );
        if (Object.keys(parsed.proof.dkim).length !== desired.domains.length)
          unavailable();
        for (const domain of desired.domains) {
          const keys = parsed.proof.dkim[domain.id];
          if (
            !keys ||
            new Set(keys.map((key) => key.selector)).size !== keys.length
          )
            unavailable();
          await db.query(
            "UPDATE webdock_mail.domain SET dkim_keys=$3 WHERE customer_id=$1 AND id=$2",
            [row.customer_id, domain.id, JSON.stringify(keys)],
          );
        }
        if (desired.submission) {
          if (!parsed.proof.submission) unavailable();
          await completeSubmissionConfiguration(
            db,
            row.customer_id,
            desired.serviceRevision,
            desired.submission,
            parsed.proof.submission,
          );
        }
        await db.query(
          "UPDATE webdock_mail.configuration SET applied_revision=$2 WHERE customer_id=$1",
          [row.customer_id, row.revision],
        );
      }
      await db.query(
        "UPDATE webdock_mail.configuration_operation SET state=$2,finished_at=now() WHERE id=$1",
        [row.id, succeeded ? "succeeded" : "needs_review"],
      );
      return {
        completed: true,
        state: succeeded ? "succeeded" : "needs_review",
      };
    });
  }
  return {
    claimMailConfiguration,
    checkpointMailConfiguration,
    completeMailConfiguration,
  };
}
