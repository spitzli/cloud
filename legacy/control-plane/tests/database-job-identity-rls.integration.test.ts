import { test, expect, mock } from "bun:test";
import { Pool } from "pg";
import { randomUUID } from "node:crypto";
import {
  withDatabaseScope,
  currentDatabaseScope,
} from "@webdock/database-runtime";
import { createSetupFixture } from "./helpers/setup-fixture";

const integration = process.env.WEBDOCK_RLS_FIXTURE_FILE ? test : test.skip;
integration(
  "API agent rehydrates Clerk through existing bootstrap mapping reads and authorizes with restricted worker SQL",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Owned local RLS fixture required");
    const owner = new Pool({ connectionString: url.href, max: 1 });
    const fixture = await createSetupFixture(owner);
    const issuer = "https://fixture.clerk.accounts.dev",
      userID = "user_" + randomUUID(),
      organizationID = "org_" + randomUUID();
    const reference = {
      issuer,
      userID,
      organizationID,
      sessionID: "sess_" + randomUUID(),
    };
    const session = {
      id: reference.sessionID,
      userId: userID,
      status: "active",
      expireAt: Date.now() + 60000,
    };
    const member = {
      organization: { id: organizationID },
      role: "org:admin",
      permissions: ["org:databases:manage"],
    };
    Object.assign(process.env, {
      WEBDOCK_PROCESS_ROLE: "api",
      WEBDOCK_AUTH_PROVIDER: '{"provider":"better-auth"}',
      WEBDOCK_CONSOLE_AUTH_PROVIDER: "clerk",
      CLERK_ISSUER: issuer,
      WEBDOCK_CONSOLE_ORIGIN: "https://console.example.test",
      WEBDOCK_CONSOLE_SERVICE_KEY: "fixture".repeat(8),
      CLERK_SECRET_KEY: "fixture-only",
      CLERK_PUBLISHABLE_KEY: "fixture-only",
    });
    mock.module("@clerk/backend", () => ({
      createClerkClient: () => ({
        sessions: { getSession: async () => session },
        users: {
          getUser: async () => ({
            id: userID,
            banned: false,
            locked: false,
            primaryEmailAddressId: "email",
            emailAddresses: [
              { id: "email", verification: { status: "verified" } },
            ],
          }),
          getOrganizationMembershipList: async () => ({
            totalCount: 1,
            data: [member],
          }),
        },
      }),
    }));
    const { authDatabase, workerDatabase, closeDatabases } =
      await import("../src/db/runtime");
    const { revalidateStoredConsoleIdentity } =
      await import("../src/modules/auth/clerk-provider");
    const { withStoredConsolePrincipal, currentConsolePrincipal } =
      await import("../src/modules/auth/principal");
    const { authorizeHosting } =
      await import("../src/modules/hosting/authorization");
    try {
      const org = (
        await owner.query(
          "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
          [fixture.customerID],
        )
      ).rows[0].organization_id;
      await owner.query(
        "INSERT INTO webdock_auth.clerk_identity(issuer,clerk_user_id,user_id) VALUES($1,$2,$3)",
        [issuer, userID, fixture.actor.subject],
      );
      await owner.query(
        "INSERT INTO webdock_auth.clerk_organization(issuer,clerk_organization_id,organization_id) VALUES($1,$2,$3)",
        [issuer, organizationID, org],
      );
      await owner.query("DELETE FROM webdock_auth.session WHERE id=$1", [
        fixture.actor.sessionID,
      ]);
      const actor = {
        ...fixture.actor,
        sessionID: reference.sessionID,
        consoleIdentity: reference,
      };
      await withDatabaseScope(
        { kind: "system", purpose: "agent", clusterID: fixture.clusterID },
        async () => {
          expect(
            (await authDatabase.query("SELECT current_user AS role")).rows[0]
              .role,
          ).toBe("webdock_auth_bootstrap");
          const db = await workerDatabase.connect();
          try {
            await db.query("BEGIN");
            expect(
              (await db.query("SELECT current_user AS role")).rows[0].role,
            ).toBe("webdock_worker_runtime");
            expect(
              (
                await db.query(
                  "SELECT id FROM webdock_auth.session WHERE id=$1",
                  [actor.sessionID],
                )
              ).rows,
            ).toEqual([]);
            const authorize = async () => {
              const principal =
                await revalidateStoredConsoleIdentity(reference);
              expect(principal.userID).toBe(actor.subject);
              expect(principal.sessionID).toBe(actor.sessionID);
              return withStoredConsolePrincipal(principal, () =>
                authorizeHosting(
                  actor,
                  {
                    permission: "org:databases:manage",
                    projectID: fixture.projectID,
                    write: true,
                    tenantAdmin: true,
                  },
                  db,
                ),
              );
            };
            await expect(
              authorizeHosting(
                actor,
                {
                  permission: "org:databases:manage",
                  projectID: fixture.projectID,
                  write: true,
                  tenantAdmin: true,
                },
                db,
              ),
            ).rejects.toThrow();
            expect((await authorize()).canWrite).toBe(true);
            expect(currentDatabaseScope()?.kind).toBe("system");
            expect(currentConsolePrincipal()).toBeUndefined();
            session.status = "revoked";
            await expect(authorize()).rejects.toThrow();
            session.status = "active";
            session.expireAt = 1;
            await expect(authorize()).rejects.toThrow();
            session.expireAt = Date.now() + 60000;
            member.permissions = [];
            await expect(authorize()).rejects.toThrow();
            member.permissions = ["org:databases:manage"];
            await owner.query(
              "UPDATE webdock_auth.clerk_identity SET enabled=false WHERE issuer=$1 AND clerk_user_id=$2",
              [issuer, userID],
            );
            await expect(authorize()).rejects.toThrow();
          } finally {
            await db.query("ROLLBACK");
            db.release();
          }
        },
      );
    } finally {
      await closeDatabases();
      await owner.query(
        "DELETE FROM webdock_auth.clerk_identity WHERE issuer=$1 AND clerk_user_id=$2",
        [issuer, userID],
      );
      await owner.query(
        "DELETE FROM webdock_auth.clerk_organization WHERE issuer=$1 AND clerk_organization_id=$2",
        [issuer, organizationID],
      );
      await fixture.cleanup();
      await owner.end();
      mock.restore();
    }
  },
);
