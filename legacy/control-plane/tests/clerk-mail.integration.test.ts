import { configureFullInstallation } from "../src/full-installation";
import { test, expect, mock } from "bun:test";
import { Pool } from "pg";
import { randomBytes, generateKeyPairSync } from "node:crypto";
import { sealSecret } from "@webdock/mail-core/secrets";
import { mailInstanceDemand } from "@webdock/mail-core/capacity";
import { withDatabaseScope } from "@webdock/database-runtime";
import { withConsolePrincipal } from "../src/modules/auth/principal";
import { database, workerDatabase } from "../src/db/runtime";
import { transaction } from "../src/modules/hosting/db";

const fixture = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(fixture ? test : test.skip)(
  "Clerk-only native mailbox grants require explicit live membership, applied revision, and worker revalidation",
  async () => {
    configureFullInstallation();
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Isolated fixture required");
    const owner = new Pool({ connectionString: url.href, max: 1 });
    const id = () =>
      "92" + BigInt("0x" + randomBytes(5).toString("hex")).toString();
    const admin = id(),
      member = id(),
      outsider = id(),
      customer = id(),
      other = id(),
      mailbox = id(),
      domain = id(),
      sender = id();
    const issuer = "https://fixture.clerk.accounts.dev";
    const savedIssuer = process.env.CLERK_ISSUER;
    const savedProvider = process.env.WEBDOCK_AUTH_PROVIDER;
    const selectedProvider = JSON.stringify({ provider: "clerk", issuer, authorizedParties: ["https://console.example.test"], audience: "webdock" });
    process.env.WEBDOCK_AUTH_PROVIDER = selectedProvider;
    process.env.CLERK_ISSUER = issuer;
    let memberPresent = true,
      memberID = "orgmem_original",
      memberRole = "org:member",
      providerBanned = false,
      providerFailure = false,
      providerOperator = false;
    let org = "";
    const actualProvider = await import("../src/modules/auth/clerk-provider");
    mock.module("../src/modules/auth/clerk-provider", () => ({
      ...actualProvider,
      clerkProvider: () => ({
        users: {
          getUser: async (userID: string) => {
            if (providerFailure) throw Error("Provider unavailable");
            return {
              id: userID,
              privateMetadata: providerOperator && userID === "user_" + outsider ? { webdockRole: "operator" } : {},
              banned: providerBanned && userID === "user_" + member,
              locked: false,
              primaryEmailAddressId: "primary",
              emailAddresses: [
                { id: "primary", verification: { status: "verified" } },
              ],
            };
          },
        },
        organizations: {
          getOrganizationMembershipList: async () => ({
            totalCount: memberPresent ? 2 : 1,
            data: [admin, ...(memberPresent ? [member] : [])].map((u) => ({
              role: u === admin ? "org:admin" : "org:member",
              publicUserData: { userId: "user_" + u },
            })),
          }),
        },
      }),
      liveMemberships: async (userID: string) =>
        userID === "user_" + outsider ||
        (userID === "user_" + member && !memberPresent)
          ? []
          : [
              {
                id: userID === "user_" + member ? memberID : "orgmem_admin",
                organization: { id: "org_" + org },
                role: userID === "user_" + admin ? "org:admin" : memberRole,
              },
            ],
    }));
    const { executeMailboxes } = await import("../src/modules/mail/mailboxes");
    const { authorizeMailbox, authorizeQueuedMailbox } =
      await import("../src/modules/mail/mail-access");
    const mailEnvNames = [
      "MAIL_ENCRYPTION_KEY",
      "MAIL_STALWART_IMAGE",
      "MAIL_HOSTNAME_SUFFIX",
      "WEBDOCK_NATIVE_MAILBOXES_ENABLED",
      "WEBDOCK_NATIVE_MAIL_SENDING_ENABLED",
    ];
    const oldMailEnv = Object.fromEntries(
      mailEnvNames.map((name) => [name, process.env[name]]),
    );
    let oldSettings: unknown;
    try {
      oldSettings = (
        await owner.query(
          "SELECT settings FROM webdock_auth.platform_settings WHERE id",
        )
      ).rows[0]?.settings;
      await owner.query(
        `INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{"mailEnabled":true}') ON CONFLICT(id) DO UPDATE SET settings=platform_settings.settings||excluded.settings`,
      );
      for (const user of [admin, member, outsider]) {
        await owner.query(
          `INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,$1,$2,'user',false,true,false)`,
          [user, user + "@mail.invalid"],
        );
        await owner.query(
          "INSERT INTO webdock_auth.clerk_identity(issuer,clerk_user_id,user_id,enabled,legacy_eligible) VALUES($1,$2,$3,true,false)",
          [issuer, "user_" + user, user],
        );
      }
      await owner.query(
        "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,'Mail fixture','active'),($2,'Foreign fixture','active')",
        [customer, other],
      );
      org = (
        await owner.query(
          "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
          [customer],
        )
      ).rows[0].organization_id;
      await owner.query(
        "INSERT INTO webdock_auth.clerk_organization VALUES($1,$2,$3,true)",
        [issuer, "org_" + org, org],
      );
      await owner.query(
        "INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,desired_enabled,state,revision,applied_revision) VALUES($1,$2,'fixture',true,'ready',1,1)",
        [customer, "mail-" + customer],
      );
      await owner.query(
        "INSERT INTO webdock_auth.mail_sender_domain(id,customer_id,domain,token,status) VALUES($1,$2,$3,'fixture','ownership_verified')",
        [sender, customer, customer + ".example.invalid"],
      );
      await owner.query(
        "INSERT INTO webdock_mail.domain(id,customer_id,sender_domain_id,name,state) VALUES($1,$2,$3,$4,'ready')",
        [domain, customer, sender, customer + ".example.invalid"],
      );
      await owner.query(
        "INSERT INTO webdock_mail.mailbox(id,customer_id,domain_id,local_part,name,quota_bytes,provider_id,state) VALUES($1,$2,$3,'hello','Hello',1048576,'account','ready')",
        [mailbox, customer, domain],
      );
      const principal = (subject: string) => ({
        provider: "clerk" as const,
        issuer,
        userID: subject,
        clerkUserID: "user_" + subject,
        sessionID: "sess_" + subject,
        operator: subject === outsider,
        organizationID: org,
        clerkOrganizationID: "org_" + org,
        customerID: customer,
        role: subject === admin ? ("admin" as const) : ("member" as const),
        verifiedAt: Date.now(),
        expiresAt: Date.now() + 59000,
        name: "Mail",
        email: subject + "@mail.invalid",
      });
      const actor = (subject: string) => ({
        subject,
        sessionID: "sess_" + subject,
      });
      const run = (subject: string, input: unknown) =>
        withConsolePrincipal(principal(subject), () =>
          transaction((db) =>
            executeMailboxes(db, actor(subject), customer, input),
          ),
        );
      const open = () =>
        withConsolePrincipal(principal(member), () =>
          transaction((db) =>
            authorizeMailbox(db, actor(member), customer, mailbox),
          ),
        );
      const queued = (revision = "1") =>
        withDatabaseScope(
          { kind: "system", purpose: "mail-webmail-gateway" },
          () =>
            workerDatabase.connect().then(async (db) => {
              try {
                return await authorizeQueuedMailbox(db, {
                  subject: member,
                  customerID: customer,
                  mailboxID: mailbox,
                  grantRevision: revision,
                });
              } finally {
                db.release();
              }
            }),
        );
      expect(
        (await database.query("SELECT current_user AS role")).rows[0].role,
      ).toBe("webdock_auth_runtime");
      expect(
        ((await run(admin, { action: "list" })) as any).people
          .map((u: any) => u.id)
          .sort(),
      ).toEqual([admin, member].sort());
      await expect(open()).rejects.toThrow();
      await expect(
        run(member, { action: "grant", mailboxID: mailbox, subject: member }),
      ).rejects.toThrow();
      await expect(
        run(admin, { action: "grant", mailboxID: mailbox, subject: outsider }),
      ).rejects.toThrow();
      await run(admin, {
        action: "grant",
        mailboxID: mailbox,
        subject: member,
      });
      const grant = (
        await owner.query(
          "SELECT * FROM webdock_mail.mailbox_grant WHERE mailbox_id=$1",
          [mailbox],
        )
      ).rows[0];
      expect(grant.member_id).toBeNull();
      expect(grant.operator_grant).toBe(false);
      expect(JSON.parse(grant.clerk_grant)).toEqual([
        issuer,
        "user_" + member,
        "org_" + org,
        memberID,
      ]);
      expect(
        (
          await owner.query(
            'SELECT count(*)::int AS n FROM webdock_auth.member WHERE "userId"=ANY($1::text[])',
            [[admin, member]],
          )
        ).rows[0].n,
      ).toBe(0);
      await expect(open()).rejects.toThrow();
      await owner.query(
        "UPDATE webdock_mail.identity SET provider_id='principal',state='ready' WHERE customer_id=$1",
        [customer],
      );
      await owner.query(
        "UPDATE webdock_mail.mailbox_grant SET applied_revision=revision WHERE mailbox_id=$1",
        [mailbox],
      );
      expect((await open()).accountID).toBe("account");
      expect((await queued()).accountID).toBe("account");
      memberRole = "org:mail_user";
      expect((await queued()).accountID).toBe("account");
      memberRole = "org:member";
      await expect(queued("2")).rejects.toThrow();
      await expect(
        withConsolePrincipal(principal(member), () =>
          transaction((db) =>
            authorizeMailbox(db, actor(member), other, mailbox),
          ),
        ),
      ).rejects.toThrow();
      process.env.CLERK_ISSUER = "https://wrong.clerk.accounts.dev";
      process.env.WEBDOCK_AUTH_PROVIDER = JSON.stringify({ ...JSON.parse(selectedProvider), issuer: process.env.CLERK_ISSUER });
      await expect(queued()).rejects.toThrow("not configured");
      process.env.CLERK_ISSUER = issuer;
      process.env.WEBDOCK_AUTH_PROVIDER = selectedProvider;
      providerOperator = true;
      await owner.query(
        `UPDATE webdock_auth."user" SET role='operator' WHERE id=$1`,
        [outsider],
      );
      const openOperator = () =>
        withConsolePrincipal(principal(outsider), () =>
          transaction((db) =>
            authorizeMailbox(db, actor(outsider), customer, mailbox),
          ),
        );
      await expect(openOperator()).rejects.toThrow();
      await run(outsider, {
        action: "grant",
        mailboxID: mailbox,
        subject: outsider,
      });
      await owner.query(
        "UPDATE webdock_mail.identity SET provider_id='operator-principal',state='ready' WHERE customer_id=$1 AND subject=$2",
        [customer, outsider],
      );
      await owner.query(
        "UPDATE webdock_mail.mailbox_grant SET applied_revision=revision WHERE mailbox_id=$1 AND subject=$2",
        [mailbox, outsider],
      );
      expect((await openOperator()).accountID).toBe("account");
      providerOperator = false;
      await expect(openOperator()).rejects.toThrow();
      providerOperator = true;
      memberPresent = false;
      await expect(open()).rejects.toThrow();
      await expect(queued()).rejects.toThrow();
      memberPresent = true;
      memberID = "orgmem_rejoined";
      await expect(open()).rejects.toThrow();
      await expect(queued()).rejects.toThrow();
      memberID = "orgmem_original";
      providerBanned = true;
      await expect(queued()).rejects.toThrow();
      providerBanned = false;
      providerFailure = true;
      await expect(queued()).rejects.toThrow();
      providerFailure = false;
      await owner.query(
        'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
        [member],
      );
      await expect(queued()).rejects.toThrow();
      await owner.query(
        'UPDATE webdock_auth."user" SET banned=false WHERE id=$1',
        [member],
      );
      await owner.query(
        "UPDATE webdock_auth.clerk_identity SET enabled=false WHERE user_id=$1",
        [member],
      );
      await expect(queued()).rejects.toThrow();
      await owner.query(
        "UPDATE webdock_auth.clerk_identity SET enabled=true WHERE user_id=$1",
        [member],
      );
      await owner.query(
        "UPDATE webdock_auth.clerk_organization SET enabled=false WHERE organization_id=$1",
        [org],
      );
      await expect(queued()).rejects.toThrow();
      await owner.query(
        "UPDATE webdock_auth.clerk_organization SET enabled=true WHERE organization_id=$1",
        [org],
      );
      await run(admin, {
        action: "revoke",
        mailboxID: mailbox,
        subject: member,
      });
      await expect(queued()).rejects.toThrow();
      // An offline cutover preserves an existing explicit legacy grant, never a disabled one.
      const legacyMember = (
        await owner.query(
          `INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,'member',now()) RETURNING id`,
          [org, member],
        )
      ).rows[0].id;
      await owner.query(
        "UPDATE webdock_mail.mailbox_grant SET member_id=$3,clerk_grant=NULL,operator_grant=false,enabled=true,revision=revision+1 WHERE mailbox_id=$1 AND subject=$2",
        [mailbox, member, legacyMember],
      );
      await owner.query(
        "UPDATE webdock_mail.mailbox_grant SET clerk_grant=NULL,operator_grant=true,enabled=false,revision=revision+1 WHERE mailbox_id=$1 AND subject=$2",
        [mailbox, outsider],
      );
      const { migrateClerkMailGrants } =
        await import("../scripts/clerk-mail-grants");
      const provider = {
        users: {
          getUser: async () => ({
            banned: false,
            locked: false,
            primaryEmailAddressId: "primary",
            emailAddresses: [
              { id: "primary", verification: { status: "verified" } },
            ],
          }),
          getOrganizationMembershipList: async () => ({
            totalCount: memberPresent ? 1 : 0,
            data: memberPresent
              ? [
                  {
                    id: memberID,
                    organization: { id: "org_" + org },
                    role: "org:member",
                  },
                ]
              : [],
          }),
        },
      } as any;
      const migrate = (dryRun: boolean) =>
        migrateClerkMailGrants({
          db: owner,
          issuer,
          users: new Map([
            [member, "user_" + member],
            [outsider, "user_" + outsider],
          ]),
          teams: [{ organizationID: org, clerkOrganizationID: "org_" + org }],
          provider,
          dryRun,
        });
      const readGrant = async () =>
        (
          await owner.query(
            "SELECT * FROM webdock_mail.mailbox_grant WHERE mailbox_id=$1 AND subject=$2",
            [mailbox, member],
          )
        ).rows[0];
      const before = await readGrant();
      memberPresent = false;
      expect(
        (await migrate(false)).find((r) => r.subject === member)?.reason,
      ).toContain("membership removed");
      expect(await readGrant()).toEqual(before);
      memberPresent = true;
      expect(
        (await migrate(true)).find((r) => r.subject === member)?.action,
      ).toBe("convert");
      expect(await readGrant()).toEqual(before);
      const report = await migrate(false);
      expect(report.find((r) => r.subject === member)?.action).toBe(
        "converted",
      );
      expect(report.find((r) => r.subject === outsider)?.reason).toBe(
        "disabled grant retained",
      );
      const converted = await readGrant();
      expect(converted.enabled).toBe(true);
      expect(converted.member_id).toBeNull();
      expect(converted.operator_grant).toBe(false);
      expect(BigInt(converted.revision)).toBe(BigInt(before.revision) + 1n);
      expect(converted.applied_revision).toBe(before.applied_revision);
      expect(JSON.parse(converted.clerk_grant)).toEqual([
        issuer,
        "user_" + member,
        "org_" + org,
        memberID,
      ]);
      expect(
        (
          await owner.query(
            "SELECT count(*)::int AS n FROM webdock_mail.configuration_operation WHERE customer_id=$1 AND reason='clerk_grants_migrated' AND state='pending' AND requested_by IS NULL",
            [customer],
          )
        ).rows[0].n,
      ).toBe(1);
      await expect(open()).rejects.toThrow();
      const clusterID = id();
      const agent = {
        clusterID,
        generation: 1,
        credentialHash: randomBytes(32).toString("hex"),
      };
      const encryptionKey = randomBytes(32).toString("base64");
      Object.assign(process.env, {
        MAIL_ENCRYPTION_KEY: encryptionKey,
        MAIL_STALWART_IMAGE: `webdock.local/mail-stalwart@sha256:${"a".repeat(64)}`,
        MAIL_HOSTNAME_SUFFIX: "mail.example.invalid",
        WEBDOCK_NATIVE_MAILBOXES_ENABLED: "true",
        WEBDOCK_NATIVE_MAIL_SENDING_ENABLED: "false",
      });
      await owner.query(
        "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,country,region,location_evidence,verified,capacity) VALUES($1,'Mail migration fixture','Contabo','DE','Germany','Isolated test',true,$2)",
        [clusterID, JSON.stringify(mailInstanceDemand)],
      );
      await owner.query(
        "INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_seen,observation) VALUES($1,$2,1,now(),$3)",
        [
          clusterID,
          agent.credentialHash,
          JSON.stringify({
            version: "v1.36.5+k3s1",
            capabilities: { nativeMail: { configurationVersion: 3 } },
          }),
        ],
      );
      await owner.query(
        "UPDATE webdock_mail.service SET host_id=$2 WHERE customer_id=$1",
        [customer, clusterID],
      );
      await owner.query(
        "INSERT INTO webdock_mail.cluster_reservation(customer_id,cluster_id,demand,status) VALUES($1,$2,$3,'active')",
        [customer, clusterID, JSON.stringify(mailInstanceDemand)],
      );
      await owner.query(
        "INSERT INTO webdock_mail.instance(customer_id,encrypted_credentials,verified_at) VALUES($1,$2,now())",
        [
          customer,
          sealSecret(
            { username: "fixture-admin", password: "synthetic-password" },
            `instance:${customer}`,
            encryptionKey,
          ),
        ],
      );
      const identities = (
        await owner.query(
          "SELECT id FROM webdock_mail.identity WHERE customer_id=$1",
          [customer],
        )
      ).rows;
      for (const identity of identities)
        await owner.query(
          "UPDATE webdock_mail.identity SET encrypted_credentials=$2 WHERE id=$1",
          [
            identity.id,
            sealSecret(
              {
                username: `wd-${identity.id}@mail-${customer}.mail.example.invalid`,
                password: "synthetic-password",
              },
              `identity:${customer}:${identity.id}`,
              encryptionKey,
            ),
          ],
        );
      const { claimMailConfiguration, completeMailConfiguration } =
        await import("../src/modules/mail/mail-configuration");
      const agentScope = {
        kind: "system" as const,
        purpose: "agent" as const,
        clusterID,
      };
      expect(
        (
          await withDatabaseScope({ ...agentScope, clusterID: other }, () =>
            workerDatabase.query(
              "SELECT id FROM webdock_auth.mail_sender_domain WHERE id=$1",
              [sender],
            ),
          )
        ).rowCount,
      ).toBe(0);
      const claim = await withDatabaseScope(agentScope, () =>
        claimMailConfiguration(agent),
      );
      expect(claim?.customerID).toBe(customer);
      expect(claim?.domains.map((d) => d.id)).toEqual([domain]);
      expect(claim?.identities.find((i) => i.subject === member)).toMatchObject(
        { enabled: true, mailboxIds: [mailbox] },
      );
      expect(
        claim?.identities.find((i) => i.subject === outsider),
      ).toMatchObject({ enabled: false, mailboxIds: [] });
      const snapshot = (
        await owner.query(
          "SELECT desired FROM webdock_mail.configuration_operation WHERE id=$1",
          [claim!.operationID],
        )
      ).rows[0].desired;
      expect(
        snapshot.identities.find((i: any) => i.subject === member),
      ).toMatchObject({ enabled: true, mailboxIds: [mailbox] });
      const { publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
      await withDatabaseScope(agentScope, () =>
        completeMailConfiguration(agent, {
          operationID: claim!.operationID,
          leaseToken: claim!.leaseToken,
          generation: 1,
          outcome: "succeeded",
          proof: {
            domains: { [domain]: "fixture-domain" },
            mailboxes: { [mailbox]: "account" },
            identities: Object.fromEntries(
              claim!.identities.map((i) => [i.id, i.providerId]),
            ),
            dkim: {
              [domain]: [
                {
                  id: "fixture-key",
                  selector: "webdock-rsa-fixture",
                  publicKey: publicKey
                    .export({ type: "spki", format: "der" })
                    .toString("base64"),
                },
              ],
            },
          },
        }),
      );
      expect((await readGrant()).applied_revision).toBe(converted.revision);
      expect((await open()).accountID).toBe("account");
      expect((await queued(String(converted.revision))).accountID).toBe(
        "account",
      );
      expect(
        (await migrate(false)).find((r) => r.subject === member)?.action,
      ).toBe("unchanged");
      expect((await readGrant()).revision).toBe(converted.revision);
      memberID = "orgmem_after_cutover";
      expect(
        (await migrate(false)).find((r) => r.subject === member)?.action,
      ).toBe("unchanged");
      await expect(open()).rejects.toThrow();
    } finally {
      mock.restore();
      for (const [name, value] of Object.entries(oldMailEnv)) {
        if (value === undefined) delete process.env[name];
        else process.env[name] = value;
      }
      if (savedProvider === undefined) delete process.env.WEBDOCK_AUTH_PROVIDER;
      else process.env.WEBDOCK_AUTH_PROVIDER = savedProvider;
      if (savedIssuer === undefined) delete process.env.CLERK_ISSUER;
      else process.env.CLERK_ISSUER = savedIssuer;
      if (oldSettings)
        await owner.query(
          "UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id",
          [oldSettings],
        );
      await owner.end();
    }
  },
);
