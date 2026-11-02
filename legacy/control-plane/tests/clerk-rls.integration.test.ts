import { configureFullInstallation } from "../src/full-installation";
configureFullInstallation();
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";
import { createHash, randomBytes } from "node:crypto";
import { Pool } from "pg";
import { withDatabaseScope } from "@webdock/database-runtime";
import { currentConsolePrincipal, withConsolePrincipal } from "../src/modules/auth/principal";
import { createDatabases } from "../src/db/client";
const fixtureFile = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(fixtureFile ? test : test.skip)(
  "Clerk RLS uses live verified scope without a legacy session",
  async () => {
    const url = new URL(
      process.env.API_TEST_OWNER_URL || process.env.DATABASE_URL || "",
    );
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Only isolated fixture allowed");
    const fixture = JSON.parse(await readFile(fixtureFile!, "utf8"));
    const owner = new Pool({ connectionString: url.href, max: 1 });
    const pools = createDatabases({
      environment: "test",
      businessURL: fixture.roles.webdock_auth_runtime,
      authURL: fixture.roles.webdock_auth_bootstrap,
      workerURL: fixture.roles.webdock_worker_runtime,
      signingSecret: fixture.contextSecret,
      max: 1,
    });
    const id = () =>
      "91" + BigInt("0x" + randomBytes(5).toString("hex")).toString();
    const user = id(),
      customer = id(),
      other = id(),
      issuer = "https://fixture.clerk.accounts.dev";
    const storageID="clerk-storage-"+user;
    const previousStudioOrigin = process.env.WEBDOCK_STUDIO_ORIGIN;
    process.env.WEBDOCK_STUDIO_ORIGIN = "https://studio.example.invalid";
    try {
      const constraintClient = await owner.connect();
      try {
        const constraint = (
          await constraintClient.query(
            "SELECT pg_get_constraintdef(oid) AS definition FROM pg_constraint WHERE conrelid='webdock_auth.managed_database_launch'::regclass AND conname='managed_database_launch_identity_provider'",
          )
        ).rows[0].definition;
        await constraintClient.query(
          "CREATE TEMP TABLE clerk_viewer_check(session_id text,clerk_identity jsonb)",
        );
        await constraintClient.query(
          "ALTER TABLE clerk_viewer_check ADD " + constraint,
        );
        for (const identity of [
          null,
          { issuer: null, userID: null, sessionID: null, organizationID: null },
          { issuer: 1, userID: 2, sessionID: 3, organizationID: null },
        ])
          await expect(
            constraintClient.query(
              "INSERT INTO clerk_viewer_check VALUES(NULL,$1)",
              [identity],
            ),
          ).rejects.toMatchObject({ code: "23514" });
        await constraintClient.query(
          "INSERT INTO clerk_viewer_check VALUES($1,NULL)",
          ["legacy"],
        );
        await constraintClient.query(
          "INSERT INTO clerk_viewer_check VALUES(NULL,$1)",
          [
            {
              issuer: "https://fixture.test",
              userID: "user",
              sessionID: "session",
              organizationID: null,
            },
          ],
        );
      } finally {
        constraintClient.release();
      }
      await owner.query(
        `INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword") VALUES($1,'Clerk fixture',$2,'user',true,false)`,
        [user, user + "@example.invalid"],
      );
      await owner.query(
        `INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,'Clerk fixture','active'),($2,'Other fixture','active')`,
        [customer, other],
      );
      const org = (
        await owner.query(
          "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
          [customer],
        )
      ).rows[0].organization_id;
      await owner.query(
        "INSERT INTO webdock_auth.clerk_identity VALUES($1,$2,$3,true)",
        [issuer, "user_" + user, user],
      );
      await owner.query(
        "INSERT INTO webdock_auth.clerk_organization VALUES($1,$2,$3,true)",
        [issuer, "org_" + org, org],
      );
      const {legacyClerkMemberships, linkedMembershipRole} = await import('../src/modules/auth/clerk-provider');
      const originalIssuer = process.env.CLERK_ISSUER;
      try {
        process.env.CLERK_ISSUER = 'https://wrong-instance.clerk.accounts.dev';
        expect(await legacyClerkMemberships(user)).toBeUndefined();
        expect(await linkedMembershipRole(user, org)).toBeUndefined();
        delete process.env.CLERK_ISSUER;
        expect(await linkedMembershipRole(user, org)).toBeUndefined();
      } finally {
        if (originalIssuer === undefined) delete process.env.CLERK_ISSUER;
        else process.env.CLERK_ISSUER = originalIssuer;
      }
      const lock = await pools.authentication.connect();
      try {
        await lock.query("BEGIN");
        const projected = await lock.query(
          `SELECT u.id FROM webdock_auth.clerk_identity i JOIN webdock_auth."user" u ON u.id=i.user_id WHERE i.issuer=$1 AND i.clerk_user_id=$2 AND i.enabled AND NOT coalesce(u.banned,false) AND u.role='user' FOR SHARE OF u`,
          [issuer, "user_" + user],
        );
        expect(projected.rows[0].id).toBe(user);
        expect(
          (
            await owner.query(
              'SELECT count(*)::int AS n FROM webdock_auth.member WHERE "userId"=$1',
              [user],
            )
          ).rows[0].n,
        ).toBe(0);
      } finally {
        await lock.query("ROLLBACK");
        lock.release();
      }
      const projectionArgs=[issuer,"user_invited_"+user,"org_"+org,"Invited fixture","invited-"+user+"@example.invalid"];
      const projected=(await pools.authentication.query('SELECT webdock_rls.project_clerk_user($1,$2,$3,$4,$5) AS id',projectionArgs)).rows[0].id;
      expect((await pools.authentication.query('SELECT webdock_rls.project_clerk_user($1,$2,$3,$4,$5) AS id',projectionArgs)).rows[0].id).toBe(projected);
      expect((await owner.query('SELECT count(*)::int AS n FROM webdock_auth.account WHERE "userId"=$1',[projected])).rows[0].n).toBe(0);
      await expect(pools.authentication.query('SELECT webdock_rls.project_clerk_user($1,$2,$3,$4,$5)',[issuer,"user_conflict_"+user,"org_"+org,"Collision",user+"@example.invalid"])).rejects.toMatchObject({code:"23505"});
      const {createAuthentication}=await import('../src/modules/auth/auth');
      const legacyAuth=createAuthentication({database:pools.authentication,baseURL:'http://localhost:3125',secret:process.env.BETTER_AUTH_SECRET!});
      const context=await legacyAuth.$context;
      await expect(context.internalAdapter.createAccount({userId:projected,providerId:'credential',accountId:projected,password:'not-a-real-hash'})).rejects.toThrow('Use your Console account');
      await expect(context.internalAdapter.createSession(projected)).rejects.toThrow('Use your Console account');
      expect((await owner.query('SELECT legacy_eligible FROM webdock_auth.clerk_identity WHERE user_id=$1',[projected])).rows[0].legacy_eligible).toBe(false);
      const scope = {
        kind: "session" as const,
        subject: user,
        sessionID: "sess_" + user,
        clerk: {
          issuer,
          userID: "user_" + user,
          operator: false,
          organizationID: org,
          clerkOrganizationID: "org_" + org,
          customerID: customer,
          role: "admin" as const,
          verifiedAt: Date.now(),
          expiresAt: Date.now() + 59000,
        },
      };
      const permissions = ["org:projects:manage", "org:deployments:create", "org:databases:manage", "org:billing:manage"];
      const capability = () => pools.business.query("SELECT webdock_rls.customer_permission($1,'org:projects:manage') AS projects, webdock_rls.customer_permission($1,'org:databases:manage') AS databases, webdock_rls.customer_permission($1,'org:deployments:create') AS deployments, webdock_rls.billing_admin($1) AS billing", [customer]);
      const denied = { projects: false, databases: false, deployments: false, billing: false };
      expect((await withDatabaseScope(scope, capability)).rows[0]).toEqual(denied);
      for (const [index, key] of ["projects", "deployments", "databases", "billing"].entries()) {
        const granted = { ...scope, clerk: { ...scope.clerk, role: "member" as const, permissions: [permissions[index]] } };
        expect((await withDatabaseScope(granted, capability)).rows[0]).toEqual({ ...denied, [key]: true });
        expect((await withDatabaseScope({ ...granted, clerk: { ...granted.clerk, permissions: [] } }, capability)).rows[0]).toEqual(denied);
      }
      const connectionID = "permission-" + user;
      const connectionWrite = () => pools.business.query("INSERT INTO webdock_auth.git_connection(id,customer_id,installation_id,account_id,account_login,permissions) VALUES($1::text,$2,$1::text,$1::text,'fixture','{}')", [connectionID, customer]);
      await expect(withDatabaseScope({ ...scope, clerk: { ...scope.clerk, permissions: ["org:deployments:create"] } }, connectionWrite)).rejects.toMatchObject({ code: "42501" });
      await withDatabaseScope({ ...scope, clerk: { ...scope.clerk, role: "member", permissions: ["org:projects:manage"] } }, connectionWrite);
      const connectionUpdate = () => pools.business.query("UPDATE webdock_auth.git_connection SET revision=revision+1 WHERE id=$1 RETURNING id", [connectionID]);
      await expect(withDatabaseScope({ ...scope, clerk: { ...scope.clerk, permissions: [] } }, connectionUpdate)).rejects.toMatchObject({ code: "42501" });
      expect((await withDatabaseScope({ ...scope, clerk: { ...scope.clerk, permissions: ["org:projects:manage"] } }, connectionUpdate)).rowCount).toBe(1);
      const projectID = id(), sourceID = id(), buildID = id();
      await owner.query("INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1,'Permission fixture',$2,'active')", [projectID, customer]);
      await owner.query("INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,mode) VALUES($1,$2,'vercel','selfservice')", [projectID, customer]);
      await owner.query("INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'fixture','{}')", [connectionID, customer]);
      await owner.query("INSERT INTO webdock_auth.git_source(id,customer_id,project_id,connection_id,repository_id,branch,root_directory,recipe,target_id) VALUES($1,$2,$3,$4,'fixture','main','','vercel',$3)", [sourceID, customer, projectID, connectionID]);
      const projects = { ...scope, clerk: { ...scope.clerk, role: "member" as const, permissions: ["org:projects:manage"] } };
      const deployments = { ...scope, clerk: { ...scope.clerk, role: "member" as const, permissions: ["org:deployments:create"] } };
      const insertBuild = () => pools.business.query("INSERT INTO webdock_auth.git_build(id,customer_id,project_id,source_id,source_sha,source_revision,connection_generation,environment_revision,subject,idempotency_key) VALUES($1::text,$2,$3,$4,$5,1,1,1,$6,$1::text)", [buildID, customer, projectID, sourceID, "a".repeat(40), user]);
      await expect(withDatabaseScope(projects, insertBuild)).rejects.toMatchObject({ code: "42501" });
      await withDatabaseScope(deployments, insertBuild);
      expect((await withDatabaseScope(deployments, () => pools.business.query("UPDATE webdock_auth.git_source SET latest_build_id=$2 WHERE id=$1 RETURNING id", [sourceID, buildID]))).rowCount).toBe(1);
      await expect(withDatabaseScope(deployments, () => pools.business.query("UPDATE webdock_auth.git_source SET branch='other' WHERE id=$1", [sourceID]))).rejects.toMatchObject({ code: "42501" });
      const invalidate = () => pools.business.query("SELECT webdock_rls.cancel_obsolete_git_builds()");
      await withDatabaseScope(projects, invalidate);
      expect((await owner.query("SELECT status FROM webdock_auth.git_build WHERE id=$1", [buildID])).rows[0].status).toBe("queued");
      await withDatabaseScope(projects, () => pools.business.query("UPDATE webdock_auth.git_source SET revision=revision+1 WHERE id=$1", [sourceID]));
      await withDatabaseScope({ ...scope, clerk: { ...scope.clerk, permissions: ["org:billing:manage"] } }, invalidate);
      expect((await owner.query("SELECT status FROM webdock_auth.git_build WHERE id=$1", [buildID])).rows[0].status).toBe("queued");
      await withDatabaseScope(projects, invalidate);
      expect((await owner.query("SELECT status,failure_code FROM webdock_auth.git_build WHERE id=$1", [buildID])).rows[0]).toEqual({ status: "cancelled", failure_code: "SOURCE_CHANGED" });
      await owner.query("DELETE FROM webdock_auth.git_check_outbox WHERE build_id=$1", [buildID]);
      await owner.query("UPDATE webdock_auth.git_source SET latest_build_id=NULL WHERE id=$1", [sourceID]);
      await owner.query("DELETE FROM webdock_auth.git_build WHERE id=$1", [buildID]);
      await owner.query("DELETE FROM webdock_auth.git_source WHERE id=$1", [sourceID]);
      await owner.query("DELETE FROM webdock_auth.git_repository WHERE connection_id=$1", [connectionID]);
      const planID = id();
      await owner.query("INSERT INTO webdock_auth.hosting_database_plan(id,plan,availability) VALUES($1,'{}','public')", [planID]);
      const databases = { ...scope, clerk: { ...scope.clerk, role: "member" as const, permissions: ["org:databases:manage"] } };
      const createOffer = () => pools.business.query("INSERT INTO webdock_auth.hosting_database_offer(id,customer_id,project_id,plan_revision_id,name,snapshot) VALUES($1,$2,$3,$4,'Permission offer','{}')", [id(), customer, projectID, planID]);
      await expect(withDatabaseScope(projects, createOffer)).rejects.toMatchObject({ code: "42501" });
      await withDatabaseScope(databases, createOffer);
      await expect(withDatabaseScope({ ...databases, clerk: { ...databases.clerk, permissions: [] } }, createOffer)).rejects.toMatchObject({ code: "42501" });
      await owner.query("DELETE FROM webdock_auth.hosting_database_offer WHERE project_id=$1", [projectID]);
      await owner.query("DELETE FROM webdock_auth.hosting_database_plan WHERE id=$1", [planID]);
      const billing = { ...scope, clerk: { ...scope.clerk, role: "member" as const, permissions: ["org:billing:manage"] } };
      const createBilling = () => pools.business.query("INSERT INTO webdock_auth.billing_customer(environment,customer_id,billing_email) VALUES('live',$1,'fixture@example.invalid')", [customer]);
      await expect(withDatabaseScope(projects, createBilling)).rejects.toMatchObject({ code: "42501" });
      await withDatabaseScope(billing, createBilling);
      const updateBilling = () => pools.business.query("UPDATE webdock_auth.billing_customer SET billing_email='updated@example.invalid' WHERE customer_id=$1 RETURNING customer_id", [customer]);
      await expect(withDatabaseScope({ ...scope, clerk: { ...scope.clerk, permissions: [] } }, updateBilling)).rejects.toMatchObject({ code: "42501" });
      expect((await withDatabaseScope(billing, updateBilling)).rowCount).toBe(1);
      await owner.query("DELETE FROM webdock_auth.hosting_project WHERE project_id=$1", [projectID]);
      await owner.query("DELETE FROM webdock_admin.projects WHERE id=$1", [projectID]);
      await owner.query("DELETE FROM webdock_auth.git_connection WHERE id=$1", [connectionID]);
      const allowed = () =>
        pools.business.query(
          "SELECT webdock_rls.customer_allowed($1,true) AS own,webdock_rls.customer_allowed($2) AS other,webdock_rls.operator() AS operator",
          [customer, other],
        );
      expect((await withDatabaseScope(scope, allowed)).rows[0]).toEqual({
        own: true,
        other: false,
        operator: false,
      });
      expect(
        (
          await withDatabaseScope(
            { ...scope, clerk: { ...scope.clerk, expiresAt: 1 } },
            allowed,
          )
        ).rows[0].own,
      ).toBe(false);
      expect(
        (
          await withDatabaseScope(
            {
              ...scope,
              clerk: { ...scope.clerk, issuer: "https://wrong.test" },
            },
            allowed,
          )
        ).rows[0].own,
      ).toBe(false);
      expect(
        (await withDatabaseScope({ ...scope, clerk: undefined }, allowed))
          .rows[0].own,
      ).toBe(false);
      const legacyID = "legacy_" + user;
      await owner.query(
        `INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval '1 hour',now())`,
        [legacyID, user],
      );
      await owner.query(
        `INSERT INTO webdock_auth.member("userId","organizationId",role,"createdAt") VALUES($1,$2,'admin',now())`,
        [user, org],
      );
      const legacy = {
        kind: "session" as const,
        subject: user,
        authProvider: "clerk" as const,
        sessionID: legacyID,
      };
      const memberships = {
        issuer,
        userID: "user_" + user,
        operator: false,
        twoFactorEnabled: false,
        subject: user,
        sessionID: legacyID,
        verifiedAt: Date.now(),
        expiresAt: Date.now() + 59000,
        roles: [{ organizationID: org, customerID: customer, role: "admin" }],
      };
      expect((await withDatabaseScope({ ...legacy, memberships }, capability)).rows[0]).toEqual(denied);
      expect((await withDatabaseScope({ ...legacy, memberships: { ...memberships, roles: [{ ...memberships.roles[0], role: "member", permissions: ["org:databases:manage"] }] } }, capability)).rows[0]).toEqual({ ...denied, databases: true });
      const otherOrg = (await owner.query("SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1", [other])).rows[0].organization_id;
      await owner.query(`INSERT INTO webdock_auth.member("userId","organizationId",role,"createdAt") VALUES($1,$2,'admin',now())`, [user, otherOrg]);
      expect((await withDatabaseScope({ ...legacy, authProvider: "better-auth" }, () => pools.business.query("SELECT webdock_rls.customer_permission($1,'org:databases:manage') AS allowed", [other]))).rows[0].allowed).toBe(true);
      expect((await withDatabaseScope(legacy, allowed)).rows[0].own).toBe(
        false,
      );
      expect(
        (await withDatabaseScope({ ...legacy, memberships }, allowed)).rows[0]
          .own,
      ).toBe(true);
      expect(
        (
          await withDatabaseScope(
            {
              ...legacy,
              memberships: {
                ...memberships,
                roles: [
                  { organizationID: org, customerID: customer, role: "member" },
                ],
              },
            },
            allowed,
          )
        ).rows[0].own,
      ).toBe(false);
      expect(
        (
          await withDatabaseScope(
            { ...legacy, memberships: { ...memberships, roles: [] } },
            allowed,
          )
        ).rows[0].own,
      ).toBe(false);
      // Provider authority remains independent of local role and MFA projections.
      const legacyOperator = { ...legacy, memberships: { ...memberships, operator: true, twoFactorEnabled: true, roles: [] } };
      expect((await withDatabaseScope(legacyOperator, allowed)).rows[0].operator).toBe(true);
      for (const patch of [
        { operator: false }, { twoFactorEnabled: false }, { expiresAt: 1 },
        { issuer: "https://wrong.test" }, { userID: "other" }, { subject: "other" }, { sessionID: "other" },
      ]) expect((await withDatabaseScope({ ...legacyOperator, memberships: { ...legacyOperator.memberships, ...patch } }, allowed)).rows[0].operator).toBe(false);
      await owner.query(`UPDATE webdock_auth."user" SET role='operator',"twoFactorEnabled"=true WHERE id=$1`, [user]);
      expect((await withDatabaseScope({ ...legacy, memberships }, allowed)).rows[0].operator).toBe(false);
      expect((await withDatabaseScope(legacy, allowed)).rows[0].operator).toBe(false);
      const { signedDatabaseScope } = await import("../src/db/signed-scope");
      const signedPools = createDatabases({environment:"test",businessURL:fixture.roles.webdock_auth_runtime,authURL:fixture.roles.webdock_auth_bootstrap,workerURL:fixture.roles.webdock_worker_runtime,signingSecret:fixture.contextSecret,max:1,scope:signedDatabaseScope});
      const previousProvider = process.env.WEBDOCK_AUTH_PROVIDER;
      try {
        process.env.WEBDOCK_AUTH_PROVIDER = JSON.stringify({provider:"clerk",issuer,authorizedParties:["https://console.example.invalid"],tokenMode:"session"});
        const previousOAuth = process.env.WEBDOCK_OAUTH_PROVIDER;
        process.env.WEBDOCK_OAUTH_PROVIDER = "better-auth";
        try {
          const { selectedIdentityDirectory } = await import("../src/modules/auth/principal");
          const read = () => selectedIdentityDirectory.session(signedPools.business, {subject: user, sessionID: legacyID});
          expect((await withDatabaseScope({...legacy, memberships}, read))?.role).toBe("user");
          expect((await withDatabaseScope(legacyOperator, read))?.role).toBe("operator");
          expect(await withDatabaseScope({...legacy, memberships:{...memberships,sessionID:"wrong"}},read)).toBeNull();
          await owner.query('UPDATE webdock_auth.clerk_identity SET enabled=false WHERE user_id=$1',[user]);
          expect(await withDatabaseScope({...legacy,memberships},read)).toBeNull();
          await owner.query('UPDATE webdock_auth.clerk_identity SET enabled=true WHERE user_id=$1',[user]);
        } finally {
          if (previousOAuth === undefined) delete process.env.WEBDOCK_OAUTH_PROVIDER;
          else process.env.WEBDOCK_OAUTH_PROVIDER = previousOAuth;
        }
        const direct = {kind:"session" as const,subject:user,sessionID:legacyID,authProvider:"better-auth" as const};
        expect((await withDatabaseScope(direct,()=>signedPools.business.query("SELECT webdock_rls.operator() AS operator"))).rows[0].operator).toBe(false);
        expect((await withDatabaseScope({...direct,memberships:{...memberships,operator:true,twoFactorEnabled:true}},()=>signedPools.business.query("SELECT webdock_rls.operator() AS operator"))).rows[0].operator).toBe(true);
      } finally {
        if(previousProvider === undefined) delete process.env.WEBDOCK_AUTH_PROVIDER; else process.env.WEBDOCK_AUTH_PROVIDER = previousProvider;
        await signedPools.close();
      }
      expect((await withDatabaseScope({ ...legacy, authProvider: "better-auth" }, allowed)).rows[0].operator).toBe(true);
      expect((await withDatabaseScope({ ...legacy, memberships }, () => pools.business.query("SELECT webdock_rls.customer_permission($1,'org:databases:manage') AS allowed", [other]))).rows[0].allowed).toBe(false);
      await owner.query('DELETE FROM webdock_auth.member WHERE "userId"=$1', [user]);
      const compatibility = Bun.spawnSync([process.execPath, "--no-env-file", "-e", `
        import { mock } from "bun:test";
        import assert from "node:assert/strict";
        import { withDatabaseScope } from "@webdock/database-runtime";
        import { betterAuthIdentity } from "@webdock/auth-better-auth";
        const subject = ${JSON.stringify(user)}, sessionID = ${JSON.stringify(legacyID)}, customerID = ${JSON.stringify(customer)};
        const principal = await import("./src/modules/auth/principal");
        mock.module("./src/modules/auth/principal", () => ({ ...principal, selectedIdentityDirectory: {
          ...principal.selectedIdentityDirectory,
          session: async (db, reference, options) => {
            const local = await betterAuthIdentity.session(db, reference, options);
            return local ? { ...local, role: "user", emailVerified: true, mustChangePassword: false, twoFactorEnabled: false } : null;
          },
        } }));
        mock.module("./src/modules/auth/business-identity", () => ({
          getBusinessIdentity: async () => ({ provider: "clerk", sessionID, user: { id: subject, email: "fixture@example.invalid" } }),
          businessOperator: async () => ({ disabled: true }),
        }));
        const { getTenant, listTenants } = await import("./src/modules/access/tenants");
        const { closeDatabases } = await import("./src/db/runtime");
        let proof = ${JSON.stringify({ ...memberships, roles: [{ organizationID: org, customerID: customer, role: "member", permissions: ["org:projects:manage"] }] })};
        const run = work => withDatabaseScope({ kind: "session", authProvider: "clerk", subject, sessionID, memberships: proof }, work);
        try {
          assert.equal((await run(() => getTenant(new Headers(), customerID))).tenant.id, customerID);
          assert.deepEqual((await run(() => listTenants(new Headers()))).map(row => row.id), [customerID]);
          proof = { ...proof, roles: [] };
          await assert.rejects(run(() => getTenant(new Headers(), customerID)));
          assert.deepEqual(await run(() => listTenants(new Headers())), []);
        } finally { await closeDatabases(); }
      `], { cwd: import.meta.dir + "/..", env: { ...process.env,
        WEBDOCK_AUTH_PROVIDER: JSON.stringify({ provider: "clerk", issuer, authorizedParties: ["https://console.example.invalid"], tokenMode: "session" }),
        CLERK_ISSUER: issuer,
      } });
      expect(compatibility.exitCode, compatibility.stderr.toString()).toBe(0);
      await owner.query(`UPDATE webdock_auth."user" SET role='user',"twoFactorEnabled"=false WHERE id=$1`, [user]);
      await owner.query("DELETE FROM webdock_auth.session WHERE id=$1", [
        legacyID,
      ]);
      await owner.query('DELETE FROM webdock_auth.member WHERE "userId"=$1', [
        user,
      ]);
      await owner.query(
        'UPDATE webdock_auth."user" SET "emailVerified"=false,"mustChangePassword"=true WHERE id=$1',
        [user],
      );
      const { getTenant } = await import("../src/modules/access/tenants");
      const view = await withConsolePrincipal(
        {
          provider: "clerk",
          issuer,
          userID: user,
          clerkUserID: "user_" + user,
          sessionID: scope.sessionID,
          operator: false,
          organizationID: org,
          clerkOrganizationID: "org_" + org,
          customerID: customer,
          role: "admin",
          verifiedAt: scope.clerk.verifiedAt,
          expiresAt: scope.clerk.expiresAt,
          name: "Fixture",
          email: user + "@example.invalid",
        },
        async () => {
          return getTenant(new Headers(), customer);
        },
      );
      expect(view.tenant.id).toBe(customer);
      await owner.query("DELETE FROM webdock_auth.billing_customer WHERE environment='live' AND customer_id=$1", [customer]);
      await owner.query(
        "UPDATE webdock_auth.clerk_identity SET enabled=false WHERE user_id=$1",
        [user],
      );
      expect((await withDatabaseScope(scope, allowed)).rows[0].own).toBe(false);
      await owner.query(
        "UPDATE webdock_auth.clerk_identity SET enabled=true WHERE user_id=$1",
        [user],
      );
      const operatorScope = { ...scope, clerk: { ...scope.clerk, operator: true } };
      for (const patch of [{ operator: undefined }, { operator: "true" }, { expiresAt: 1 }, { issuer: "https://wrong.test" }, { userID: "other" }]) {
        expect((await withDatabaseScope({ ...operatorScope, clerk: { ...operatorScope.clerk, ...patch } } as typeof scope, allowed)).rows[0].operator).toBe(false);
      }
      // Raw context tampering cannot create a signed authority proof.
      expect((await pools.business.query("SELECT set_config('webdock.rls_context',$1,false),webdock_rls.operator() AS operator", [JSON.stringify({ scope: operatorScope })])).rows[0].operator).toBe(false);
      expect((await withDatabaseScope(operatorScope, allowed)).rows[0].operator).toBe(true);
      await owner.query("INSERT INTO webdock_auth.tenant_storage(id,customer_id,label,environment,store_id,prefix,encrypted_token) VALUES($1,$2,'Clerk fixture','production',$3,'','fixture-only')",[storageID,customer,"store-"+user]);
      const {manageTenantStorage}=await import('../src/modules/platform/storage-usage');
      await withConsolePrincipal({provider:'clerk',issuer,userID:user,clerkUserID:'user_'+user,sessionID:scope.sessionID,operator:true,organizationID:null,clerkOrganizationID:null,customerID:null,role:null,verifiedAt:Date.now(),expiresAt:Date.now()+59000,name:'Operator fixture',email:user+'@example.invalid'},()=>manageTenantStorage(new Headers(),customer,{action:'remove',storageID}));
      expect((await owner.query('SELECT count(*)::int AS n FROM webdock_auth.tenant_storage WHERE id=$1',[storageID])).rows[0].n).toBe(0);
      const { startTenantPreview, currentTenantPreview, exitTenantPreview } = await import("../src/modules/access/tenant-preview");
      const previewActor = { userID: user, sessionID: scope.sessionID };
      const previewPrincipal = {
        provider: "clerk" as const, issuer, userID: user, clerkUserID: "user_" + user,
        sessionID: scope.sessionID, operator: true, organizationID: null,
        clerkOrganizationID: null, customerID: null, role: null,
        verifiedAt: Date.now(), expiresAt: Date.now() + 59000,
        name: "Operator fixture", email: user + "@example.invalid",
      };
      const preview = await withConsolePrincipal(previewPrincipal, () => startTenantPreview(previewActor, customer));
      expect(preview.customerID).toBe(customer);
      expect((await withConsolePrincipal(previewPrincipal, () => currentTenantPreview(previewActor)))?.preview.status).toBe("active");
      await expect(withConsolePrincipal(previewPrincipal, () => exitTenantPreview({ ...previewActor, sessionID: "spoofed" }))).rejects.toThrow();
      expect((await withConsolePrincipal(previewPrincipal, () => exitTenantPreview(previewActor))).customerID).toBe(customer);
      await owner.query(
        `UPDATE webdock_auth."user" SET role='operator' WHERE id=$1`,
        [user],
      );
      const memberPrincipal = { ...previewPrincipal, operator: false, organizationID: org, clerkOrganizationID: "org_" + org, customerID: customer, role: "member" as const };
      const { listTenants } = await import("../src/modules/access/tenants");
      const { getTenantPlan } = await import("../src/modules/plans/plans");
      expect((await withConsolePrincipal(memberPrincipal, () => getTenant(new Headers(), customer))).tenant.id).toBe(customer);
      expect((await withConsolePrincipal(memberPrincipal, () => listTenants(new Headers()))).map(row => row.id)).toEqual([customer]);
      expect(await withConsolePrincipal(memberPrincipal, () => getTenantPlan(new Headers(), customer))).toHaveProperty("offers");
      const { getOffer, acceptOffer } = await import("../src/modules/plans/plans");
      const offerToken = randomBytes(32).toString("base64url"), offerID = id();
      const allowance = { storageBytes: 0, mailMessages: 0, transferBytes: 0, websites: 0, editors: 0 };
      await owner.query("INSERT INTO webdock_auth.plan_offer(id,customer_id,token_hash,name,description,terms,base,offered_extras,expected_revision,expires_at,created_by) VALUES($1,$2,$3,'Clerk billing fixture','','',$4,$4,0,now()+interval '1 day',$5)", [offerID, customer, createHash("sha256").update(offerToken).digest("hex"), allowance, user]);
      try {
        expect((await withConsolePrincipal(memberPrincipal, () => getOffer(new Headers(), offerToken))).canAccept).toBe(false);
        const billingPrincipal = { ...memberPrincipal, permissions: ["org:billing:manage" as const] };
        expect((await withConsolePrincipal(billingPrincipal, () => getOffer(new Headers(), offerToken))).canAccept).toBe(true);
        await expect(withConsolePrincipal(memberPrincipal, () => acceptOffer(new Headers(), offerToken))).rejects.toThrow();
        expect((await withConsolePrincipal(billingPrincipal, () => acceptOffer(new Headers(), offerToken))).customerID).toBe(customer);
      } finally {
        await owner.query("DELETE FROM webdock_auth.plan_offer WHERE id=$1", [offerID]);
        await owner.query("DELETE FROM webdock_auth.subscription_history WHERE customer_id=$1", [customer]);
        await owner.query("DELETE FROM webdock_auth.tenant_subscription WHERE customer_id=$1", [customer]);
      }
      await expect(withConsolePrincipal({ ...previewPrincipal, operator: false }, () => startTenantPreview(previewActor, customer))).rejects.toThrow();
      expect((await owner.query("SELECT count(*)::int AS n FROM webdock_auth.console_tenant_preview WHERE issuer=$1 AND session_id=$2", [issuer, scope.sessionID])).rows[0].n).toBe(0);

      expect((await withDatabaseScope(scope, allowed)).rows[0].operator).toBe(
        false,
      );
      await owner.query(
        "INSERT INTO webdock_auth.console_tenant_preview(issuer,session_id,actor_id,customer_id,organization_id,expires_at) VALUES($1,$2,$3,$4,$5,now()-interval '1 minute')",
        [issuer, scope.sessionID, user, customer, org],
      );
      expect((await withDatabaseScope(scope, allowed)).rows[0]).toEqual({
        own: false,
        other: false,
        operator: false,
      });
      expect((await withDatabaseScope({ ...scope, clerk: { ...scope.clerk, permissions } }, capability)).rows[0]).toEqual(denied);
      expect(
        (
          await owner.query(
            'SELECT count(*)::int AS n FROM webdock_auth.session WHERE "userId"=$1',
            [user],
          )
        ).rows[0].n,
      ).toBe(0);
    } finally {
      if (previousStudioOrigin === undefined) delete process.env.WEBDOCK_STUDIO_ORIGIN;
      else process.env.WEBDOCK_STUDIO_ORIGIN = previousStudioOrigin;
      await owner.query("DELETE FROM webdock_auth.tenant_storage WHERE id=$1 AND label='Clerk fixture' AND encrypted_token='fixture-only'",[storageID]);
      await pools.close();
      await owner.end();
    }
  },
);
