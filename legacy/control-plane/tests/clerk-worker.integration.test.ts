import { test, expect } from "bun:test";
import { Pool } from "pg";
import { randomUUID } from "node:crypto";
import { withDatabaseScope } from "@webdock/database-runtime";
import { createSetupFixture } from "./helpers/setup-fixture";
import { executeHosting } from "../src/modules/hosting/service";
import { workerDatabase } from "../src/db/runtime";
const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "Clerk worker requires a fresh proof, enabled mapping and no legacy session",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Local fixture required");
    const db = new Pool({ connectionString: url.href, max: 1 }),
      f = await createSetupFixture(db),
      old = process.env.WEBDOCK_SETUP_ENABLED,
      oldProvider = process.env.WEBDOCK_AUTH_PROVIDER;
    let savedID: string | undefined;
    process.env.WEBDOCK_SETUP_ENABLED = "true";
    try {
      const saved = await executeHosting(f.actor, {
        action: "setup.save",
        projectID: f.projectID,
        revision: 0,
        configuration: {
          name: "clerk-worker",
          source: { kind: "template", template: "healthcheck" },
          spec: {
            args: [],
            port: 8080,
            healthPath: "/ping",
            cpuMillicores: 100,
            memoryBytes: 67108864,
            ephemeralBytes: 67108864,
            replicas: 1,
          },
          database: { mode: "none" },
          mail: null,
          autoPublish: false,
        },
        idempotencyKey: randomUUID(),
      });
      savedID = saved.id;
      const review = await executeHosting(f.actor, {
        action: "setup.preview",
        projectID: f.projectID,
        setupID: saved.id,
        revision: saved.revision,
      });
      await executeHosting(f.actor, {
        action: "setup.confirm",
        projectID: f.projectID,
        setupID: saved.id,
        revision: saved.revision,
        previewID: review.preview.id,
        acceptedOfferIDs: [],
        idempotencyKey: randomUUID(),
      });
      const org = (
        await db.query(
          "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
          [f.customerID],
        )
      ).rows[0].organization_id;
      const reference = {
        issuer: "https://fixture.clerk.accounts.dev",
        userID: "user_" + f.actor.subject,
        sessionID: "sess_" + f.actor.subject,
        organizationID: "org_" + org,
      };
      await db.query(
        "INSERT INTO webdock_auth.clerk_identity VALUES($1,$2,$3,true)",
        [reference.issuer, reference.userID, f.actor.subject],
      );
      await db.query(
        "INSERT INTO webdock_auth.clerk_organization VALUES($1,$2,$3,true)",
        [reference.issuer, reference.organizationID, org],
      );
      const actor = {
        ...f.actor,
        sessionID: reference.sessionID,
        consoleIdentity: reference,
      };
      const row = (
        await db.query(
          "UPDATE webdock_auth.hosting_setup SET actor=$2,generation=generation+1,lease_until=now()+interval '60 seconds' WHERE id=$1 RETURNING *",
          [saved.id, actor],
        )
      ).rows[0];
      await db.query("DELETE FROM webdock_auth.session WHERE id=$1", [
        f.actor.sessionID,
      ]);
      await db.query(
        'UPDATE webdock_auth."user" SET "twoFactorEnabled"=false,"emailVerified"=false,"mustChangePassword"=true WHERE id=$1',
        [f.actor.subject],
      );
      const scope = {
        kind: "system" as const,
        purpose: "hosting-setup",
        customerID: f.customerID,
        setupID: saved.id,
        generation: row.generation,
        clerk: {
          issuer: reference.issuer,
          userID: reference.userID,
          operator: false,
          subject: f.actor.subject,
          sessionID: reference.sessionID,
          organizationID: org,
          clerkOrganizationID: reference.organizationID,
          customerID: f.customerID,
          role: "admin" as const,
          permissions: [
            "org:projects:manage",
            "org:deployments:create",
            "org:databases:manage",
            "org:billing:manage",
          ],
          verifiedAt: Date.now(),
          expiresAt: Date.now() + 59000,
        },
      };
      const check = () =>
        workerDatabase.query(
          "SELECT webdock_rls.setup_allowed($1,$2) AS allowed",
          [f.customerID, f.projectID],
        );
      expect((await withDatabaseScope(scope, check)).rows[0].allowed).toBe(
        true,
      );
      expect(
        (
          await withDatabaseScope(
            { ...scope, clerk: { ...scope.clerk, permissions: [] } },
            check,
          )
        ).rows[0].allowed,
      ).toBe(false);
      expect(
        (
          await withDatabaseScope(
            {
              ...scope,
              clerk: {
                ...scope.clerk,
                role: "member",
                permissions: ["org:projects:manage"],
              },
            },
            check,
          )
        ).rows[0].allowed,
      ).toBe(true);
      expect(
        (
          await withDatabaseScope(
            {
              ...scope,
              clerk: { ...scope.clerk, permissions: ["org:databases:manage"] },
            },
            check,
          )
        ).rows[0].allowed,
      ).toBe(false);
      await db.query(
        "UPDATE webdock_auth.hosting_setup SET configuration=jsonb_set(configuration,'{database,mode}',to_jsonb('create'::text)) WHERE id=$1",
        [saved.id],
      );
      expect(
        (
          await withDatabaseScope(
            {
              ...scope,
              clerk: { ...scope.clerk, permissions: ["org:projects:manage"] },
            },
            check,
          )
        ).rows[0].allowed,
      ).toBe(false);
      expect(
        (
          await withDatabaseScope(
            {
              ...scope,
              clerk: {
                ...scope.clerk,
                permissions: ["org:projects:manage", "org:databases:manage"],
              },
            },
            check,
          )
        ).rows[0].allowed,
      ).toBe(true);
      await db.query(
        "UPDATE webdock_auth.hosting_setup SET configuration=jsonb_set($2::jsonb,'{source,kind}',to_jsonb('git'::text)) WHERE id=$1",
        [saved.id, row.configuration],
      );
      expect(
        (
          await withDatabaseScope(
            {
              ...scope,
              clerk: { ...scope.clerk, permissions: ["org:projects:manage"] },
            },
            check,
          )
        ).rows[0].allowed,
      ).toBe(false);
      expect(
        (
          await withDatabaseScope(
            {
              ...scope,
              clerk: {
                ...scope.clerk,
                permissions: ["org:projects:manage", "org:deployments:create"],
              },
            },
            check,
          )
        ).rows[0].allowed,
      ).toBe(true);
      await db.query(
        "UPDATE webdock_auth.hosting_setup SET configuration=$2 WHERE id=$1",
        [saved.id, row.configuration],
      );
      expect(
        (
          await withDatabaseScope(
            { ...scope, clerk: { ...scope.clerk, expiresAt: 1 } },
            check,
          )
        ).rows[0].allowed,
      ).toBe(false);
      expect(
        (await withDatabaseScope({ ...scope, clerk: undefined }, check)).rows[0]
          .allowed,
      ).toBe(false);
      await db.query(
        "UPDATE webdock_auth.clerk_identity SET enabled=false WHERE user_id=$1",
        [f.actor.subject],
      );
      expect((await withDatabaseScope(scope, check)).rows[0].allowed).toBe(
        false,
      );
      // Exercise the real entry point in a process which has only worker credentials.
      const revalidationScope = {
        kind: "system" as const,
        purpose: "hosting-setup-revalidation",
        customerID: f.customerID,
        setupID: saved.id,
        generation: row.generation,
      };
      const mapping = () =>
        workerDatabase.query(
          "SELECT webdock_rls.setup_clerk_revalidation($1,$2,$3,$4) AS mapping",
          [
            reference.issuer,
            reference.userID,
            reference.sessionID,
            reference.organizationID,
          ],
        );
      expect(
        (
          await withDatabaseScope(
            { ...revalidationScope, customerID: "wrong-customer" },
            mapping,
          )
        ).rows[0].mapping,
      ).toBeNull();
      expect(
        (
          await withDatabaseScope(revalidationScope, () =>
            workerDatabase.query(
              "SELECT webdock_rls.setup_clerk_revalidation($1,$2,$3,$4) AS mapping",
              [
                reference.issuer,
                "user_wrong",
                reference.sessionID,
                reference.organizationID,
              ],
            ),
          )
        ).rows[0].mapping,
      ).toBeNull();
      expect(
        (
          await withDatabaseScope(
            { ...revalidationScope, generation: row.generation + 1 },
            mapping,
          )
        ).rows[0].mapping,
      ).toBeNull();
      await db.query(
        "UPDATE webdock_auth.hosting_setup SET lease_until=now()-interval '1 second' WHERE id=$1",
        [saved.id],
      );
      expect(
        (await withDatabaseScope(revalidationScope, mapping)).rows[0].mapping,
      ).toBeNull();
      const child = async (
        status: string,
        mismatch = false,
        permissions = [
          "org:projects:manage",
          "org:databases:manage",
          "org:deployments:create",
        ],
        role = "org:admin",
      ) => {
        const env: NodeJS.ProcessEnv = {
          ...process.env,
          WEBDOCK_PROCESS_ROLE: "worker",
          WEBDOCK_SETUP_ENABLED: "true",
          CLERK_ISSUER: reference.issuer,
          WEBDOCK_CONSOLE_ORIGIN: "https://console.example.invalid",
          WEBDOCK_CONSOLE_SERVICE_KEY: "fixture-service-key-32-characters-long",
          CLERK_SECRET_KEY: "sk_test_fixture",
          CLERK_PUBLISHABLE_KEY: "pk_test_fixture",
        };
        for (const key of [
          "DATABASE_URL",
          "AUTH_DATABASE_URL",
          "API_TEST_OWNER_URL",
          "DATABASE_ADMIN_URL",
        ])
          delete env[key];
        const script = `
          import { mock } from "bun:test";
          mock.module("@clerk/backend", () => ({ createClerkClient: () => ({
            sessions: { getSession: async () => ({ id: ${JSON.stringify(reference.sessionID)}, userId: ${JSON.stringify(mismatch ? "user_wrong" : reference.userID)}, status: ${JSON.stringify(status)}, expireAt: Date.now()+60000 }) },
            users: {
              getUser: async () => ({ id: ${JSON.stringify(reference.userID)}, banned: false, locked: false, primaryEmailAddressId: "email", emailAddresses: [{ id: "email", verification: { status: "verified" } }] }),
              getOrganizationMembershipList: async () => ({ data: [{organization: {id: ${JSON.stringify(reference.organizationID)}}, role: ${JSON.stringify(role)}, permissions: ${JSON.stringify(permissions)}}], totalCount: 1 })
            }
          }) }));
          const { runSetupBatch } = await import("./src/modules/hosting/setup-worker");
          const { closeDatabases } = await import("./src/db/runtime");
          try { console.log(JSON.stringify(await runSetupBatch({limit:1}))); } finally { await closeDatabases(); }
        `;
        const worker = Bun.spawn(
          [process.execPath, "--no-env-file", "-e", script],
          { cwd: import.meta.dir + "/..", env, stdout: "pipe", stderr: "pipe" },
        );
        const [output, errors, code] = await Promise.all([
          new Response(worker.stdout).text(),
          new Response(worker.stderr).text(),
          worker.exited,
        ]);
        expect(errors).toBe("");
        expect(code).toBe(0);
        return JSON.parse(output.trim());
      };
      const reset = async () =>
        db.query(
          "UPDATE webdock_auth.hosting_setup SET state='running',lease_until=NULL,next_attempt_at='1900-01-01',steps=$2,resources=$3 WHERE id=$1",
          [saved.id, JSON.stringify(row.steps), JSON.stringify(row.resources)],
        );
      await db.query(
        "UPDATE webdock_auth.clerk_identity SET enabled=true WHERE user_id=$1",
        [f.actor.subject],
      );
      await reset();
      expect(await child("active")).toEqual({ processed: 1, failed: 0 });
      expect(
        (
          await db.query(
            "SELECT resources FROM webdock_auth.hosting_setup WHERE id=$1",
            [saved.id],
          )
        ).rows[0].resources.appID,
      ).toBeTruthy();
      await reset();
      expect(await child("revoked")).toEqual({ processed: 1, failed: 1 });
      await reset();
      expect(await child("active", true)).toEqual({ processed: 1, failed: 1 });
      // A linked legacy session uses the same live provider capability, without bootstrap credentials.
      process.env.WEBDOCK_AUTH_PROVIDER = JSON.stringify({provider:"clerk",issuer:reference.issuer,authorizedParties:["https://console.example.invalid"],tokenMode:"session"});
      await db.query(
        'UPDATE webdock_auth."user" SET "emailVerified"=true,"mustChangePassword"=false WHERE id=$1',
        [f.actor.subject],
      );
      await db.query(
        `INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval '1 hour',now())`,
        [f.actor.sessionID, f.actor.subject],
      );
      await db.query(
        "UPDATE webdock_auth.hosting_setup SET actor=$2 WHERE id=$1",
        [saved.id, f.actor],
      );
      await db.query(
        `UPDATE webdock_auth.member SET role='member' WHERE "userId"=$1`,
        [f.actor.subject],
      );
      const legacyClaim = (
        await db.query(
          "UPDATE webdock_auth.hosting_setup SET state='running',lease_until=now()+interval '60 seconds' WHERE id=$1 RETURNING generation",
          [saved.id],
        )
      ).rows[0];
      const memberships = {
        operator: false,
        twoFactorEnabled: false,
        sessionID: f.actor.sessionID,
        subject: f.actor.subject,
        issuer: reference.issuer,
        userID: reference.userID,
        verifiedAt: Date.now(),
        expiresAt: Date.now() + 59000,
        roles: [
          {
            organizationID: org,
            customerID: f.customerID,
            role: "member",
            permissions: ["org:projects:manage"],
          },
        ],
      };
      const legacyScope = {
        ...scope,
        authProvider: "clerk" as const,
        generation: legacyClaim.generation,
        clerk: undefined,
        memberships,
      };
      expect(
        (await withDatabaseScope(legacyScope, check)).rows[0].allowed,
      ).toBe(true);
      expect(
        (
          await withDatabaseScope(
            { ...legacyScope, memberships: undefined },
            check,
          )
        ).rows[0].allowed,
      ).toBe(false);
      expect(
        (
          await withDatabaseScope(
            { ...legacyScope, memberships: { ...memberships, expiresAt: 1 } },
            check,
          )
        ).rows[0].allowed,
      ).toBe(false);
      expect(
        (
          await withDatabaseScope(
            {
              ...legacyScope,
              memberships: { ...memberships, subject: "wrong" },
            },
            check,
          )
        ).rows[0].allowed,
      ).toBe(false);
      await reset();
      expect(
        await child("active", false, ["org:projects:manage"], "org:member"),
      ).toEqual({ processed: 1, failed: 0 });
      await reset();
      expect(await child("active", false, [], "org:member")).toEqual({
        processed: 1,
        failed: 1,
      });
      await reset();
      expect(
        await child("active", false, ["org:databases:manage"], "org:member"),
      ).toEqual({ processed: 1, failed: 1 });
      await db.query(
        "UPDATE webdock_auth.clerk_identity SET enabled=false WHERE user_id=$1",
        [f.actor.subject],
      );
      await reset();
      expect(await child("active")).toEqual({ processed: 1, failed: 1 });
    } finally {
      if (oldProvider === undefined) delete process.env.WEBDOCK_AUTH_PROVIDER; else process.env.WEBDOCK_AUTH_PROVIDER = oldProvider;
      if (old === undefined) delete process.env.WEBDOCK_SETUP_ENABLED;
      else process.env.WEBDOCK_SETUP_ENABLED = old;
      try {
        if (savedID)
          await db.query(
            "UPDATE webdock_auth.hosting_setup SET state='blocked',lease_until=NULL WHERE id=$1 AND project_id=$2",
            [savedID, f.projectID],
          );
      } finally {
        await db.end();
      }
    }
  },
);
