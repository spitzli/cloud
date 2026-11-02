import { mailboxSchemaSQL } from "../../src/modules/mail/mailbox-schema";
import { executeMailboxes } from "../../src/modules/mail/mailboxes";
import {
  claimMailConfiguration,
  checkpointMailConfiguration,
  completeMailConfiguration,
} from "../../src/modules/mail/mail-configuration";
import { transaction } from "../../src/modules/hosting/db";
import test from "node:test";
import assert from "node:assert/strict";
import { generateKeyPairSync, randomBytes } from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import {
  nativeMailSchemaSQL,
  nativeMailClusterSchemaSQL,
} from "@webdock/mail-core/schema";
import { mailInstanceDemand } from "@webdock/mail-core/capacity";
import { getMailService, requestMailService } from "@webdock/mail-core";
import { hostingSchemaSQL } from "../../src/modules/hosting/schema";
import {
  claimNativeMail,
  checkpointNativeMail,
  completeNativeMail,
} from "../../src/modules/mail/mail-agent";
import { openSecret } from "@webdock/mail-core/secrets";

const url = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(url.hostname) ||
  url.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local database required");
test.after(async () => {
  await auth.$context;
  await database.end();
});

test("only the assigned cluster can claim, checkpoint and complete native Mail provisioning", async () => {
  await database.query(hostingSchemaSQL);
  await database.query(nativeMailSchemaSQL);
  await database.query(nativeMailClusterSchemaSQL);
  await database.query(mailboxSchemaSQL);
  const previous = {
    boxes: process.env.WEBDOCK_NATIVE_MAILBOXES_ENABLED,
    flag: process.env.WEBDOCK_NATIVE_MAIL_ENABLED,
    key: process.env.MAIL_ENCRYPTION_KEY,
    image: process.env.MAIL_STALWART_IMAGE,
    suffix: process.env.MAIL_HOSTNAME_SUFFIX,
  };
  process.env.WEBDOCK_NATIVE_MAIL_ENABLED = "true";
  process.env.WEBDOCK_NATIVE_MAILBOXES_ENABLED = "true";
  process.env.MAIL_ENCRYPTION_KEY = randomBytes(32).toString("base64");
  process.env.MAIL_STALWART_IMAGE = `webdock.local/mail-stalwart@sha256:${"a".repeat(64)}`;
  process.env.MAIL_HOSTNAME_SUFFIX = "mail.webdock.dev";
  const actor = (
    await database.query(
      'INSERT INTO webdock_auth."user"(name,email,"emailVerified",role,"mustChangePassword","twoFactorEnabled") VALUES(\'Mail agent test\',$1,true,\'operator\',false,true) RETURNING id',
      [`agent-${randomBytes(8).toString("hex")}@example.invalid`],
    )
  ).rows[0].id;
  const customerID = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Mail agent fixture') RETURNING id",
    )
  ).rows[0].id;
  const clusters = (
    await database.query(
      `INSERT INTO webdock_auth.hosting_cluster(name,provider,country,region,location_evidence,verified,capacity)
    VALUES('Mail A','Contabo','DE','Germany','Local test',true,$1),('Mail B','Contabo','DE','Germany','Local test',true,$1) RETURNING id`,
      [JSON.stringify(mailInstanceDemand)],
    )
  ).rows;
  const agents = clusters.map((cluster) => ({
    clusterID: cluster.id,
    generation: 1,
    credentialHash: randomBytes(32).toString("hex"),
  }));
  for (const agent of agents)
    await database.query(
      "INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_seen,observation) VALUES($1,$2,1,now(),$3)",
      [
        agent.clusterID,
        agent.credentialHash,
        JSON.stringify({
          version: "v1.36.5+k3s1",
          capabilities: {
            storageVersion: 1,
            nativeMail: {
              version: 1,
              configurationVersion: 3,
              image: process.env.MAIL_STALWART_IMAGE,
            },
          },
        }),
      ],
    );
  const priorSettings = (
    await database.query(
      "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  try {
    await database.query(
      "INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}') ON CONFLICT(id) DO UPDATE SET settings=excluded.settings",
    );
    await requestMailService(database, {
      customerID,
      actorID: actor,
      hostID: agents[0].clusterID,
      enabled: true,
      expectedRevision: "0",
    });
    await requestMailService(database, {
      customerID,
      actorID: actor,
      hostID: agents[0].clusterID,
      enabled: false,
      expectedRevision: "1",
    });
    await requestMailService(database, {
      customerID,
      actorID: actor,
      hostID: agents[0].clusterID,
      enabled: true,
      expectedRevision: "2",
    });
    assert.equal(await claimNativeMail(agents[1]), null);
    const packet = await claimNativeMail(agents[0]);
    assert.ok(packet);
    assert.equal(packet.customerID, customerID);
    assert.equal(packet.clusterID, agents[0].clusterID);
    assert.equal(packet.action, "ensure");
    assert.equal(packet.revision, 3);
    assert.ok(packet.credentials.bootstrapPassword);
    assert.equal(await claimNativeMail(agents[0]), null);
    const input = {
      operationID: packet.operationID,
      leaseToken: packet.leaseToken,
      generation: 1,
    };
    await assert.rejects(
      checkpointNativeMail(agents[1], {
        ...input,
        credentials: { username: "admin", password: "fixture-secret" },
      }),
    );
    await checkpointNativeMail(agents[0], {
      ...input,
      credentials: { username: "admin", password: "fixture-secret" },
    });
    const stored = (
      await database.query(
        "SELECT encrypted_credentials FROM webdock_mail.instance WHERE customer_id=$1",
        [customerID],
      )
    ).rows[0];
    assert.ok(!stored.encrypted_credentials.includes("fixture-secret"));
    assert.equal(
      openSecret<{ password: string }>(
        stored.encrypted_credentials,
        `instance:${customerID}`,
        process.env.MAIL_ENCRYPTION_KEY!,
      ).password,
      "fixture-secret",
    );
    await assert.rejects(
      checkpointNativeMail(agents[0], {
        ...input,
        credentials: { username: "admin", password: "replacement" },
      }),
    );
    await database.query(
      "UPDATE webdock_auth.hosting_agent SET generation=2 WHERE cluster_id=$1",
      [agents[0].clusterID],
    );
    await assert.rejects(
      checkpointNativeMail(
        { ...agents[0], generation: 2 },
        { ...input, generation: 2 },
      ),
    );
    await database.query(
      "UPDATE webdock_auth.hosting_agent SET generation=1 WHERE cluster_id=$1",
      [agents[0].clusterID],
    );
    const proof = {
      namespace: `wd-mail-${customerID}`,
      revision: 3,
      running: true,
      edition: "community",
      recoveryDisabled: true,
      submissionCertificateSha256: "a".repeat(64),
    };
    await assert.rejects(
      completeNativeMail(agents[1], { ...input, outcome: "succeeded", proof }),
    );
    await assert.rejects(
      completeNativeMail(agents[0], {
        ...input,
        outcome: "succeeded",
        proof: { ...proof, namespace: "kube-system" },
      }),
    );
    assert.equal(
      (await getMailService(database, customerID)).state,
      "provisioning",
    );
    await completeNativeMail(agents[0], {
      ...input,
      outcome: "succeeded",
      proof,
    });
    assert.equal((await getMailService(database, customerID)).state, "ready");
    const instance = (
      await database.query(
        "SELECT * FROM webdock_mail.instance WHERE customer_id=$1",
        [customerID],
      )
    ).rows[0];
    assert.equal(
      instance.internal_url,
      `http://app-${customerID}.wd-mail-${customerID}.svc.cluster.local:8080`,
    );
    assert.equal(
      instance.public_url,
      `https://mail-${customerID}.mail.webdock.dev`,
    );
    assert.equal(instance.smtp_certificate_sha256, "a".repeat(64));
    assert.equal(String(instance.smtp_certificate_revision), "3");
    const credentials = openSecret<Record<string, string>>(
      instance.encrypted_credentials,
      `instance:${customerID}`,
      process.env.MAIL_ENCRYPTION_KEY!,
    );
    assert.equal(credentials.bootstrapPassword, undefined);
    await assert.rejects(
      completeNativeMail(agents[0], { ...input, outcome: "succeeded", proof }),
    );
    const mailSession = (
      await database.query(
        `INSERT INTO webdock_auth.session("userId",token,"expiresAt","createdAt","updatedAt") VALUES($1,$2,now()+interval '1 hour',now(),now()) RETURNING id`,
        [actor, randomBytes(32).toString("hex")],
      )
    ).rows[0].id;
    const mailActor = { subject: actor, sessionID: mailSession };
    const manage = (command: unknown) =>
      transaction((db) => executeMailboxes(db, mailActor, customerID, command));
    const sourceID = (
      await database.query(
        "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token,status) VALUES($1,$2,'fixture','ownership_verified') RETURNING id",
        [customerID, `mail-${customerID}.example.com`],
      )
    ).rows[0].id;
    const domain = (await manage({
      action: "domain.attach",
      senderDomainID: sourceID,
    })) as { id: string };
    const mailbox = (await manage({
      action: "mailbox.create",
      domainID: domain.id,
      localPart: "support",
      name: "Support",
      quotaBytes: 1048576,
    })) as { id: string };
    await manage({ action: "grant", mailboxID: mailbox.id, subject: actor });
    assert.equal(await claimMailConfiguration(agents[1]), null);
    const configuration = await claimMailConfiguration(agents[0]);
    assert.ok(configuration);
    assert.equal(configuration.customerID, customerID);
    assert.equal(configuration.identities.length, 1);
    const identity = configuration.identities[0];
    assert.deepEqual(identity.mailboxIds, [mailbox.id]);
    const storedIdentity = (
      await database.query(
        "SELECT encrypted_credentials FROM webdock_mail.identity WHERE id=$1",
        [identity.id],
      )
    ).rows[0];
    assert.ok(
      !storedIdentity.encrypted_credentials.includes(identity.password),
    );
    const claim = {
      operationID: configuration.operationID,
      leaseToken: configuration.leaseToken,
      generation: agents[0].generation,
    };
    await assert.rejects(checkpointMailConfiguration(agents[1], claim));
    const publicKey = generateKeyPairSync("rsa", { modulusLength: 2048 })
      .publicKey.export({ format: "der", type: "spki" })
      .toString("base64");
    const configurationProof = {
      dkim: {
        [domain.id]: [
          { id: "signature", selector: "webdock-rsa-20261010", publicKey },
        ],
      },
      domains: { [domain.id]: "domain" },
      mailboxes: { [mailbox.id]: "group" },
      identities: { [identity.id]: "principal" },
    };
    await assert.rejects(
      completeMailConfiguration(agents[1], {
        ...claim,
        outcome: "succeeded",
        proof: configurationProof,
      }),
    );
    await assert.rejects(
      completeMailConfiguration(agents[0], {
        ...claim,
        outcome: "succeeded",
        proof: {
          ...configurationProof,
          identities: { [identity.id]: "group" },
        },
      }),
    );
    await manage({ action: "revoke", mailboxID: mailbox.id, subject: actor });
    await completeMailConfiguration(agents[0], {
      ...claim,
      outcome: "succeeded",
      proof: configurationProof,
    });
    assert.deepEqual(
      (
        await database.query(
          "SELECT dkim_keys FROM webdock_mail.domain WHERE id=$1",
          [domain.id],
        )
      ).rows[0].dkim_keys,
      configurationProof.dkim[domain.id],
    );
    const revoked = (
      await database.query(
        "SELECT enabled,revision,applied_revision FROM webdock_mail.mailbox_grant WHERE mailbox_id=$1",
        [mailbox.id],
      )
    ).rows[0];
    assert.equal(revoked.enabled, false);
    assert.notEqual(revoked.revision, revoked.applied_revision);
    const removal = await claimMailConfiguration(agents[0]);
    assert.ok(removal);
    assert.equal(removal.identities[0].enabled, false);
    assert.deepEqual(removal.identities[0].mailboxIds, []);
    await completeMailConfiguration(agents[0], {
      operationID: removal.operationID,
      leaseToken: removal.leaseToken,
      generation: agents[0].generation,
      outcome: "succeeded",
      proof: configurationProof,
    });
  } finally {
    for (const [name, value] of Object.entries({
      WEBDOCK_NATIVE_MAILBOXES_ENABLED: previous.boxes,
      WEBDOCK_NATIVE_MAIL_ENABLED: previous.flag,
      MAIL_ENCRYPTION_KEY: previous.key,
      MAIL_STALWART_IMAGE: previous.image,
      MAIL_HOSTNAME_SUFFIX: previous.suffix,
    })) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
    for (const table of [
      "mailbox_grant",
      "configuration_operation",
      "configuration",
      "access_event",
      "identity",
      "mailbox",
      "domain",
    ])
      await database.query(
        `DELETE FROM webdock_mail.${table} WHERE customer_id=$1`,
        [customerID],
      );
    await database.query(
      "DELETE FROM webdock_auth.mail_sender_domain WHERE customer_id=$1",
      [customerID],
    );
    await database.query(
      "DELETE FROM webdock_mail.instance WHERE customer_id=$1",
      [customerID],
    );
    await database.query(
      "DELETE FROM webdock_mail.operation WHERE customer_id=$1",
      [customerID],
    );
    await database.query(
      "DELETE FROM webdock_mail.cluster_reservation WHERE customer_id=$1",
      [customerID],
    );
    await database.query(
      "DELETE FROM webdock_mail.service WHERE customer_id=$1",
      [customerID],
    );
    await database.query(
      "DELETE FROM webdock_auth.hosting_agent WHERE cluster_id=ANY($1)",
      [agents.map((agent) => agent.clusterID)],
    );
    await database.query(
      "DELETE FROM webdock_auth.hosting_cluster WHERE id=ANY($1)",
      [agents.map((agent) => agent.clusterID)],
    );
    const map = (
      await database.query(
        "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1 RETURNING organization_id",
        [customerID],
      )
    ).rows[0];
    await database.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
      customerID,
    ]);
    await database.query("DELETE FROM webdock_auth.organization WHERE id=$1", [
      map.organization_id,
    ]);
    await database.query('DELETE FROM webdock_auth."user" WHERE id=$1', [
      actor,
    ]);
    if (priorSettings)
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id=true",
        [priorSettings.settings],
      );
    else
      await database.query(
        "DELETE FROM webdock_auth.platform_settings WHERE id=true",
      );
  }
});
