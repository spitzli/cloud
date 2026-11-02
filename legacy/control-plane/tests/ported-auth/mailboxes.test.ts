import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import { nativeMailSchemaSQL } from "@webdock/mail-core/schema";
import { mailboxSchemaSQL } from "../../src/modules/mail/mailbox-schema";
import { executeMailboxes } from "../../src/modules/mail/mailboxes";
import {
  authorizeMailbox,
  type MailActor,
} from "../../src/modules/mail/mail-access";
import { transaction } from "../../src/modules/hosting/db";
import {
  launchWebmail,
  exchangeWebmail,
  inspectWebmail,
  revokeWebmail,
} from "../../src/modules/mail/mail-sessions";
import { sealSecret } from "@webdock/mail-core/secrets";
import {
  reserveWebmailWrite,
  finishWebmailWrite,
} from "../../src/modules/mail/mail-writes";
const target = new URL(process.env.DATABASE_URL!);
if (
  target.hostname !== "127.0.0.1" ||
  target.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local database required");
test.after(async () => {
  await auth.$context;
  await database.end();
});

test("management does not grant mailbox access; explicit grants and current membership are required", async () => {
  await database.query(nativeMailSchemaSQL);
  await database.query(mailboxSchemaSQL);
  await database.query(mailboxSchemaSQL);
  const nonce = randomBytes(6).toString("hex");
  const users = (
    await database.query(
      `INSERT INTO webdock_auth."user"(name,email,"emailVerified",role,"mustChangePassword","twoFactorEnabled") VALUES
    ('Operator',$1,true,'operator',false,true),('Member',$2,true,'user',false,false),('Outsider',$3,true,'user',false,false) RETURNING id`,
      [
        nonce + "-op@example.invalid",
        nonce + "-member@example.invalid",
        nonce + "-other@example.invalid",
      ],
    )
  ).rows;
  const actors: MailActor[] = [];
  for (const user of users) {
    const session = (
      await database.query(
        `INSERT INTO webdock_auth.session("userId",token,"expiresAt","createdAt","updatedAt") VALUES($1,$2,now()+interval '1 hour',now(),now()) RETURNING id`,
        [user.id, randomBytes(32).toString("hex")],
      )
    ).rows[0];
    actors.push({ subject: user.id, sessionID: session.id });
  }
  const customerID = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Mailboxes fixture') RETURNING id",
    )
  ).rows[0].id;
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customerID],
    )
  ).rows[0].organization_id;
  await database.query(
    `INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,'admin',now())`,
    [org, actors[1].subject],
  );
  await database.query(
    `INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,desired_enabled,state,revision,applied_revision) VALUES($1,$2,'local-test',true,'ready',1,1)`,
    [customerID, "mail-" + customerID],
  );
  const sender = (
    await database.query(
      "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token,status,checked_at) VALUES($1,$2,$3,'pending',now()) RETURNING id",
      [customerID, nonce + ".example.com", nonce],
    )
  ).rows[0].id;
  const previous = (
    await database.query(
      "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  const run = (actor: MailActor, command: unknown) =>
    transaction((db) => executeMailboxes(db, actor, customerID, command));
  try {
    await database.query(
      "INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}') ON CONFLICT(id) DO UPDATE SET settings=excluded.settings",
    );
    await assert.rejects(run(actors[2], { action: "list" }));
    await assert.rejects(
      run(actors[1], { action: "domain.attach", senderDomainID: sender }),
      /ownership/,
    );
    await database.query(
      "UPDATE webdock_auth.mail_sender_domain SET status='ownership_verified' WHERE id=$1",
      [sender],
    );
    const domain = (await run(actors[1], {
      action: "domain.attach",
      senderDomainID: sender,
    })) as { id: string };
    await database.query(
      "UPDATE webdock_auth.mail_sender_domain SET deleted_at=now() WHERE id=$1",
      [sender],
    );
    await assert.rejects(
      run(actors[1], { action: "domain.attach", senderDomainID: sender }),
    );
    await assert.rejects(
      run(actors[1], {
        action: "mailbox.create",
        domainID: domain.id,
        localPart: "deleted",
        name: "Deleted",
        quotaBytes: 1048576,
      }),
    );
    await database.query(
      "UPDATE webdock_auth.mail_sender_domain SET deleted_at=NULL WHERE id=$1",
      [sender],
    );
    const mailbox = (await run(actors[1], {
      action: "mailbox.create",
      domainID: domain.id,
      localPart: "support",
      name: "Support",
      quotaBytes: 1048576,
    })) as { id: string };
    for (const actor of actors.slice(0, 2))
      await assert.rejects(
        transaction((db) =>
          authorizeMailbox(db, actor, customerID, mailbox.id),
        ),
      );
    await assert.rejects(
      run(actors[1], {
        action: "grant",
        mailboxID: mailbox.id,
        subject: actors[2].subject,
      }),
    );
    await run(actors[1], {
      action: "grant",
      mailboxID: mailbox.id,
      subject: actors[1].subject,
    });
    await database.query(
      "UPDATE webdock_mail.mailbox SET provider_id='group',state='ready' WHERE id=$1",
      [mailbox.id],
    );
    await database.query(
      "UPDATE webdock_mail.identity SET provider_id='member',state='ready' WHERE customer_id=$1",
      [customerID],
    );
    await assert.rejects(
      transaction((db) =>
        authorizeMailbox(db, actors[1], customerID, mailbox.id),
      ),
    );
    await database.query(
      "UPDATE webdock_mail.mailbox_grant SET applied_revision=revision WHERE customer_id=$1",
      [customerID],
    );
    assert.equal(
      (
        await transaction((db) =>
          authorizeMailbox(db, actors[1], customerID, mailbox.id),
        )
      ).accountID,
      "group",
    );
    process.env.WEBDOCK_NATIVE_MAILBOXES_ENABLED = "true";
    process.env.MAIL_GATEWAY_ORIGIN = "https://mail-gateway.example.com";
    process.env.MAIL_MANAGED_CLUSTER_ID = "local-test";
    process.env.MAIL_ENCRYPTION_KEY = randomBytes(32).toString("base64");
    const identity = (
      await database.query(
        "SELECT id FROM webdock_mail.identity WHERE customer_id=$1",
        [customerID],
      )
    ).rows[0];
    await database.query(
      "UPDATE webdock_mail.identity SET encrypted_credentials=$2 WHERE id=$1",
      [
        identity.id,
        sealSecret(
          { username: "private-user", password: "private-password" },
          `identity:${customerID}:${identity.id}`,
          process.env.MAIL_ENCRYPTION_KEY,
        ),
      ],
    );
    await assert.rejects(
      transaction((db) => launchWebmail(db, actors[0], customerID, mailbox.id)),
    );
    const launch = await transaction((db) =>
      launchWebmail(db, actors[1], customerID, mailbox.id),
    );
    assert.ok(!JSON.stringify(launch).includes("private-password"));
    const session = await transaction((db) => exchangeWebmail(db, launch.code));
    await assert.rejects(transaction((db) => exchangeWebmail(db, launch.code)));
    const access = await transaction((db) => inspectWebmail(db, session.token));
    assert.equal(access.accountID, "group");
    assert.equal(access.customerID, customerID);
    assert.equal(access.senderName, "Support");
    assert.equal(
      access.runtimeOrigin,
      `http://app-${customerID}.wd-mail-${customerID}.svc.cluster.local:8080`,
    );
    const write = {
      token: session.token,
      requestID: "11111111-1111-4111-8111-111111111111",
      action: "submit" as const,
      resourceID: "draft1",
      payloadHash: "a".repeat(64),
    };
    const reservation = await transaction((db) =>
      reserveWebmailWrite(db, write),
    );
    assert.equal(reservation.execute, true);
    await assert.rejects(
      transaction((db) =>
        reserveWebmailWrite(db, {
          ...write,
          action: "draft",
          requestID: "33333333-3333-4333-8333-333333333333",
        }),
      ),
    );
    assert.equal(
      (await transaction((db) => reserveWebmailWrite(db, write))).execute,
      false,
    );
    assert.equal(
      (
        await transaction((db) =>
          reserveWebmailWrite(db, {
            ...write,
            requestID: "22222222-2222-4222-8222-222222222222",
          }),
        )
      ).execute,
      false,
    );
    await assert.rejects(
      transaction((db) =>
        reserveWebmailWrite(db, { ...write, payloadHash: "b".repeat(64) }),
      ),
    );
    await transaction((db) =>
      finishWebmailWrite(db, {
        token: session.token,
        operationID: reservation.operationID,
        leaseToken: reservation.leaseToken!,
        state: "succeeded",
        result: { id: "receipt" },
      }),
    );
    assert.deepEqual(
      (await transaction((db) => reserveWebmailWrite(db, write))).result,
      { id: "receipt" },
    );
    await transaction((db) =>
      reserveWebmailWrite(db, {
        ...write,
        action: "draft",
        resourceID: "draft2",
        requestID: "44444444-4444-4444-8444-444444444444",
      }),
    );
    await assert.rejects(
      transaction((db) =>
        reserveWebmailWrite(db, {
          ...write,
          resourceID: "draft2",
          requestID: "55555555-5555-4555-8555-555555555555",
        }),
      ),
    );
    await assert.rejects(
      transaction((db) =>
        finishWebmailWrite(db, {
          token: session.token,
          operationID: reservation.operationID,
          leaseToken: reservation.leaseToken!,
          state: "uncertain",
        }),
      ),
    );
    const secondLaunch = await transaction((db) =>
      launchWebmail(db, actors[1], customerID, mailbox.id),
    );
    const logout = await transaction((db) =>
      exchangeWebmail(db, secondLaunch.code),
    );
    await transaction((db) => revokeWebmail(db, logout.token));
    await assert.rejects(transaction((db) => inspectWebmail(db, logout.token)));
    await assert.rejects(
      transaction((db) =>
        authorizeMailbox(db, actors[0], customerID, mailbox.id),
      ),
    );
    await database.query(
      'DELETE FROM webdock_auth.member WHERE "organizationId"=$1 AND "userId"=$2',
      [org, actors[1].subject],
    );
    await assert.rejects(
      transaction((db) => inspectWebmail(db, session.token)),
    );
    await assert.rejects(
      transaction((db) =>
        authorizeMailbox(db, actors[1], customerID, mailbox.id),
      ),
    );

    await database.query(
      `INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,'admin',now())`,
      [org, actors[1].subject],
    );
    await assert.rejects(
      transaction((db) =>
        authorizeMailbox(db, actors[1], customerID, mailbox.id),
      ),
    );
    assert.equal(
      (
        await database.query(
          "SELECT enabled FROM webdock_mail.mailbox_grant WHERE mailbox_id=$1",
          [mailbox.id],
        )
      ).rows[0].enabled,
      false,
    );
    await run(actors[0], {
      action: "revoke",
      mailboxID: mailbox.id,
      subject: actors[1].subject,
    });
    assert.ok(
      !JSON.stringify(await run(actors[0], { action: "list" })).includes(
        "encrypted_credentials",
      ),
    );
    assert.equal(
      (
        await database.query(
          "SELECT count(*)::int AS count FROM webdock_mail.configuration_operation WHERE customer_id=$1 AND state='pending'",
          [customerID],
        )
      ).rows[0].count,
      1,
    );
  } finally {
    for (const table of [
      "webmail_write",
      "webmail_session",
      "webmail_launch",
      "mailbox_grant",
      "configuration_operation",
      "configuration",
      "access_event",
      "identity",
      "mailbox",
      "domain",
      "service",
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
      "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customerID],
    );
    await database.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
      customerID,
    ]);
    await database.query("DELETE FROM webdock_auth.organization WHERE id=$1", [
      org,
    ]);
    await database.query('DELETE FROM webdock_auth."user" WHERE id=ANY($1)', [
      users.map((u) => u.id),
    ]);
    if (previous)
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id=true",
        [previous.settings],
      );
  }
});
