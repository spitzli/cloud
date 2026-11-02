import { readFile } from "node:fs/promises";
import { ScopedPool, withDatabaseScope } from "@webdock/database-runtime";
import { test, expect } from "bun:test";
import { Pool } from "pg";
import { randomBytes } from "node:crypto";
import { createRegistry } from "../src/modules/registry/service";
const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "registry keeps ownership, archive dependencies and atomic audit under signed operator RLS",
  async () => {
    const url = new URL(
      process.env.API_TEST_OWNER_URL || process.env.DATABASE_URL!,
    );
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Isolated registry fixture required");
    const db = new Pool({ connectionString: url.href });
    const base = BigInt(
      "94" + BigInt("0x" + randomBytes(4).toString("hex")).toString(),
    );
    const [subject, session, native, other, otherSession] = [0, 1, 2, 3, 4].map(
      (n) => (base + BigInt(n)).toString(),
    );
    const registry = await createRegistry();
    const created: string[] = [];
    try {
      const { databasePools } = await import("../src/db/runtime");
      await (
        await import("../src/db/readiness")
      ).verifyDatabaseReadiness(
        databasePools,
        ["auth", "registry", "lifecycle", "integrations"],
        registry.raw,
      );
      const { createDatabases } = await import("../src/db/client");
      const wrong = createDatabases({
        environment: "test",
        businessURL: process.env.DATABASE_URL!,
        authURL: process.env.AUTH_DATABASE_URL!,
        workerURL: process.env.DATABASE_WORKER_URL!,
        signingSecret: "wrong-fixture-context-key-at-least-32-characters",
        max: 1,
      });
      try {
        await expect(
          (await import("../src/db/readiness")).verifyDatabaseReadiness(wrong, [
            "auth",
          ]),
        ).rejects.toThrow("signature");
      } finally {
        await wrong.close();
      }
      for (const [id, role, sid] of [
        [subject, "operator", session],
        [other, "user", otherSession],
      ]) {
        await db.query(
          `INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,'Registry fixture',$2,$3,true,false,true)`,
          [id, id + "@example.invalid", role],
        );
        await db.query(
          `INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval '1 hour',now())`,
          [sid, id],
        );
      }
      await db.query(
        `INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,'Operator',$2,'operator',$3)`,
        [native, native + "@example.invalid", subject],
      );
      const actor = { subject, sessionID: session };
      await expect(
        registry.writeCustomer(
          { subject: other, sessionID: otherSession },
          null,
          { name: "Denied" },
        ),
      ).rejects.toThrow();
      const c = await registry.writeCustomer(actor, null, {
        name: "Registry customer",
        country: "de",
      });
      created.push(c.id);
      expect(c.country).toBe("DE");
      const credentials = JSON.parse(
        await readFile(process.env.WEBDOCK_RLS_FIXTURE_FILE!, "utf8"),
      );
      const portal = new ScopedPool(
        { connectionString: credentials.roles.webdock_auth_runtime, max: 1 },
        { secret: credentials.contextSecret, audience: "webdock_auth_runtime" },
      );
      try {
        await db.query(
          `INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) SELECT $1,$2,$3::varchar,organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$3`,
          [session, subject, c.id],
        );
        const mapping = await withDatabaseScope(
          { kind: "session", subject, sessionID: session },
          () => portal.query("SELECT webdock_rls.portal_user() AS value"),
        );
        expect(mapping.rows[0].value.id).toBe(native);
        expect(mapping.rows[0].value).not.toHaveProperty("hash");
        await expect(
          registry.writeCustomer(actor, c.id, { name: "Preview write" }),
        ).rejects.toThrow();
      } finally {
        await db.query(
          "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
          [session],
        );
        await portal.end();
      }
      const p = await registry.writeProject(actor, null, {
        name: "Registry project",
        customer: c.id,
      });
      const { linkGitHubRepository } =
        await import("../src/modules/integrations/github/project");
      const { closeRegistry } = await import("../src/modules/registry/context");
      try {
        await linkGitHubRepository(
          {
            identity: actor,
            user: {
              id: native,
              name: "Operator",
              email: native + "@example.invalid",
              role: "operator",
              collection: "users",
            },
          },
          {
            repository: async () => ({
              url: "https://github.com/fixture/verified",
            }),
          } as never,
          { operatorID: native } as never,
          {
            project: p.id,
            installation: 7,
            installationPage: 1,
            page: 1,
            repository: 11,
          },
        );
        expect(
          (
            (await registry.getRecord(actor, "projects", p.id)) as {
              repositoryURL: string;
            }
          ).repositoryURL,
        ).toBe("https://github.com/fixture/verified");
      } finally {
        await closeRegistry();
      }
      const { currentRegistryActor } =
        await import("../src/modules/integrations/operator");
      const {
        saveVercelConnection,
        loadVercelConnection,
        removeVercelConnection,
      } = await import("../src/modules/integrations/vercel/vercel-store");
      await withDatabaseScope(
        { kind: "session", subject, sessionID: session },
        async () => {
          const providerActor = await currentRegistryActor(),
            secret = "fixture-vercel-encryption-key-at-least-32-characters",
            credential = {
              teamID: "team_fixture" + base,
              configurationID: "icfg_fixture",
              accessToken: "fixture-token-not-in-DTOs",
            };
          try {
            await saveVercelConnection(providerActor, credential, secret);
            expect(
              (
                await db.query(
                  "SELECT encrypted_token FROM webdock_admin.vercel_connection WHERE team_id=$1",
                  [credential.teamID],
                )
              ).rows[0].encrypted_token,
            ).not.toContain(credential.accessToken);
            expect(
              await loadVercelConnection(
                providerActor,
                credential.teamID,
                secret,
              ),
            ).toEqual(credential);
            await db.query(
              'UPDATE webdock_auth."user" SET "twoFactorEnabled"=false WHERE id=$1',
              [subject],
            );
            expect(
              await loadVercelConnection(
                providerActor,
                credential.teamID,
                secret,
              ),
            ).toBeNull();
          } finally {
            await db.query(
              'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
              [subject],
            );
            await removeVercelConnection(providerActor, credential.teamID);
            await closeRegistry();
          }
        },
      );
      const { auth } = await import("../src/modules/auth/auth");
      const { serializeSignedCookie } = await import("better-call");
      const { handleNativeStudioRequest } =
        await import("../src/modules/portal/studio-api");
      const authContext = await auth.$context;
      const callRPC = async (
        token: string,
        operation: string,
        args: unknown[],
      ) => {
        const cookie = await serializeSignedCookie(
          authContext.authCookies.sessionToken.name,
          token,
          authContext.secret,
        );
        return handleNativeStudioRequest(
          new Request("http://localhost:3125/v1/auth/studio", {
            method: "POST",
            headers: {
              origin: "http://localhost:3125",
              "content-type": "application/json",
              cookie: cookie.split(";")[0],
            },
            body: JSON.stringify({ operation, args }),
          }),
        );
      };
      const throughHTTP = await callRPC(session, "registry.query", [
        "find",
        { collection: "projects", where: { id: { equals: p.id } }, depth: 1 },
      ]);
      expect(throughHTTP.status).toBe(200);
      expect((await throughHTTP.json()).data.docs[0].customer.id).toBe(c.id);
      expect(
        (
          await callRPC(otherSession, "registry.customer", [
            null,
            { name: "Unauthorized" },
          ])
        ).status,
      ).toBe(403);
      const found = await registry.query(actor, "find", {
        collection: "projects",
        where: { customer: { equals: c.id } },
        depth: 1,
        sort: ["name", "id"],
      });
      expect(found.docs[0].customer.id).toBe(c.id);
      const nativeRow = await registry.query(actor, "findByID", {
        collection: "users",
        id: native,
        depth: 0,
      });
      expect(nativeRow).not.toHaveProperty("hash");
      await expect(
        registry.query(actor, "find", {
          collection: "users",
          select: { hash: true },
        }),
      ).rejects.toThrow();
      const selected = await registry.query(actor, "findByID", {
        collection: "users",
        id: native,
        depth: 0,
        select: { name: true },
      });
      expect(Object.keys(selected).sort()).toEqual(["id", "name"]);
      await expect(
        registry.query(actor, "find", {
          collection: "projects",
          sort: "constructor",
        }),
      ).rejects.toThrow();
      await expect(
        registry.query(actor, "find", {
          collection: "projects",
          where: JSON.parse('{"constructor":{"equals":"x"}}'),
        }),
      ).rejects.toThrow();
      expect(
        (
          await registry.query(actor, "count", {
            collection: "projects",
            where: { name: { equals: "x' OR 1=1 --" } },
          })
        ).totalDocs,
      ).toBe(0);

      await expect(
        registry.archive(actor, "customers", c.id, true),
      ).rejects.toThrow("active projects");
      const instance = await registry.writeInstance(
        actor,
        null,
        {
          label: "CMS",
          project: p.id,
          adminURL: "https://cms.example.invalid",
          schemaName: "registry_" + base,
          provider: "other",
          providerProjectID: "fixture-" + base,
          template: "custom",
          status: "suspended",
        },
        true,
      );
      await expect(
        registry.archive(actor, "projects", p.id, true),
      ).rejects.toThrow("Retire");
      await expect(
        registry.writeProject(actor, p.id, { name: p.name, customer: other }),
      ).rejects.toThrow();
      await registry.writeInstance(
        actor,
        instance.id,
        {
          label: instance.label,
          project: instance.project,
          adminURL: instance.adminURL,
          schemaName: instance.schemaName,
          provider: instance.provider,
          providerProjectID: instance.providerProjectID,
          template: instance.template,
          status: "retired",
        },
        true,
      );
      await registry.archive(actor, "projects", p.id, true);
      await registry.archive(actor, "customers", c.id, true);
      expect(
        (
          await db.query(
            "SELECT count(*)::int AS n FROM webdock_admin.audit_events WHERE actor_id=$1",
            [native],
          )
        ).rows[0].n,
      ).toBe(9);
      const envKeys = [
        "WEBDOCK_VERCEL_CLIENT_ID",
        "WEBDOCK_VERCEL_CLIENT_SECRET",
        "WEBDOCK_VERCEL_INTEGRATION_SLUG",
        "WEBDOCK_VERCEL_TEAM_ID",
        "WEBDOCK_VERCEL_TEAM_SLUG",
        "WEBDOCK_SSO_COOKIE_SECRET",
        "WEBDOCK_STUDIO_ORIGIN",
        "WEBDOCK_MANAGED_DATABASES",
      ];
      const savedEnv = Object.fromEntries(
        envKeys.map((key) => [key, process.env[key]]),
      );
      const deletionCustomer = await registry.writeCustomer(actor, null, {
        name: "Deletion fixture",
      });
      created.push(deletionCustomer.id);
      const deletionProject = await registry.writeProject(actor, null, {
        name: "Delete me",
        customer: deletionCustomer.id,
        url: "https://deleteme.webdock.dev",
      });
      const schemaName = "fixturedelete" + base,
        providerProjectID = "prj_delete" + base,
        teamID = "team_delete" + base;
      await registry.writeInstance(
        actor,
        null,
        {
          label: "Delete CMS",
          project: deletionProject.id,
          adminURL: "https://studio.webdock.dev/sites/1234",
          schemaName,
          provider: "vercel",
          providerProjectID,
          template: "custom",
          status: "active",
        },
        true,
      );
      const { previewProjectDeletion, deleteProject } =
        await import("../src/modules/lifecycle/project-deletion");
      const { saveVercelLink } =
        await import("../src/modules/integrations/vercel/vercel-store");
      let hostingDeletes = 0,
        identityDeletes = 0,
        databaseDeletes = 0,
        started!: () => void;
      const startedPromise = new Promise<void>((resolve) => {
        started = resolve;
      });
      const effect = {
        fetchProject: async () => ({
          projectID: providerProjectID,
          name: "Fixture",
          framework: null,
          nodeVersion: null,
          checkedAt: new Date().toISOString(),
          deployments: [],
          domains: [
            { name: "deleteme.webdock.dev", verified: true, redirect: null },
          ],
        }),
        identity: async (_actor: unknown, input: { operation: string }) => {
          if (input.operation === "delete") identityDeletes++;
          return {
            bindingID: "1234",
            customerID: deletionCustomer.id,
            origin: "https://deleteme.webdock.dev",
            clientID: "10001",
            organizationID: "10002",
            label: "Fixture",
            planHash: "a".repeat(64),
            alreadyRemoved: identityDeletes > 0,
          };
        },
        inspectDatabase: async (schema: string, remove = false) => {
          if (remove && ++databaseDeletes === 1)
            throw Error("Synthetic interrupted database removal");
          return { schema, alreadyAbsent: false };
        },
        deleteHosting: async () => {
          hostingDeletes++;
          started();
          await Bun.sleep(21_000);
          return { status: "deleted" as const };
        },
      };
      try {
        Object.assign(process.env, {
          WEBDOCK_VERCEL_CLIENT_ID: "fixture",
          WEBDOCK_VERCEL_CLIENT_SECRET: "fixture-client-secret",
          WEBDOCK_VERCEL_INTEGRATION_SLUG: "fixture",
          WEBDOCK_VERCEL_TEAM_ID: teamID,
          WEBDOCK_VERCEL_TEAM_SLUG: "fixture",
          WEBDOCK_SSO_COOKIE_SECRET:
            "fixture-vercel-cookie-key-at-least-32-characters",
          WEBDOCK_STUDIO_ORIGIN: "https://studio.webdock.dev",
          WEBDOCK_MANAGED_DATABASES: JSON.stringify({
            [schemaName]: {
              url: `postgresql://${schemaName}_runtime:fixture@fixture.neon.tech/neondb?sslmode=verify-full`,
            },
          }),
        });
        await withDatabaseScope(
          { kind: "session", subject, sessionID: session },
          async () => {
            const deletionActor = {
              ...(await currentRegistryActor()),
              accessToken: "fixture-access-token",
            };
            await saveVercelConnection(
              deletionActor,
              {
                teamID,
                configurationID: "icfg_fixture",
                accessToken: "fixture-provider-token",
              },
              process.env.WEBDOCK_SSO_COOKIE_SECRET!,
            );
            await saveVercelLink(
              deletionActor,
              deletionProject.id,
              teamID,
              providerProjectID,
            );
            const preview = await previewProjectDeletion(
              deletionActor,
              deletionProject.id,
              effect,
            );
            const confirmation = {
              confirmName: "Delete me",
              planHash: preview.planHash,
            };
            const attempt = deleteProject(
              deletionActor,
              deletionProject.id,
              confirmation,
              effect,
            ).then(
              (result) => ({ result, error: undefined }),
              (error) => ({ result: undefined, error }),
            );
            await Promise.race([
              startedPromise,
              attempt.then((value) => {
                if (value.error) throw value.error;
              }),
            ]);
            expect(deletionActor.payload.db.pool.idleCount).toBe(
              deletionActor.payload.db.pool.totalCount,
            );
            const firstLease = (
              await db.query(
                "SELECT lease_until FROM webdock_admin.project_deletion WHERE project_id=$1",
                [deletionProject.id],
              )
            ).rows[0].lease_until;
            await expect(
              registry.patchRepository(
                actor,
                deletionProject.id,
                "https://github.com/fixture/changed",
              ),
            ).rejects.toHaveProperty("status", 409);
            await Bun.sleep(20_500);
            expect(
              (
                await db.query(
                  "SELECT lease_until>$2 AS renewed FROM webdock_admin.project_deletion WHERE project_id=$1",
                  [deletionProject.id, firstLease],
                )
              ).rows[0].renewed,
            ).toBe(true);
            expect((await attempt).error).toBeDefined();
            expect(
              (
                await db.query(
                  "SELECT completed_steps FROM webdock_admin.project_deletion WHERE project_id=$1",
                  [deletionProject.id],
                )
              ).rows[0].completed_steps,
            ).toEqual(["hosting", "identity"]);
            const resumed = await deleteProject(
              deletionActor,
              deletionProject.id,
              confirmation,
              effect,
            );
            expect(resumed.status).toBe("deleted");
            expect(hostingDeletes).toBe(1);
            expect(identityDeletes).toBe(1);
            expect(databaseDeletes).toBe(2);
            expect(
              (
                await deleteProject(
                  deletionActor,
                  deletionProject.id,
                  confirmation,
                  effect,
                )
              ).alreadyDeleted,
            ).toBe(true);
            await removeVercelConnection(deletionActor, teamID);
          },
        );
      } finally {
        for (const key of envKeys) {
          const value = savedEnv[key];
          if (value === undefined) delete process.env[key];
          else process.env[key] = value;
        }
        await db.query(
          "DELETE FROM webdock_admin.project_deletion WHERE project_id=$1",
          [deletionProject.id],
        );
        await db.query(
          "DELETE FROM webdock_admin.vercel_connection WHERE team_id=$1",
          [teamID],
        );
      }
    } finally {
      await db.query(
        "DELETE FROM webdock_admin.audit_events WHERE actor_id=$1",
        [native],
      );
      await db.query(
        "DELETE FROM webdock_admin.cms_instances WHERE project_id IN (SELECT id FROM webdock_admin.projects WHERE customer_id=ANY($1))",
        [created],
      );
      await db.query(
        "DELETE FROM webdock_admin.projects WHERE customer_id=ANY($1)",
        [created],
      );
      const orgs = (
        await db.query(
          "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1) RETURNING organization_id",
          [created],
        )
      ).rows.map((x) => x.organization_id);
      await db.query("DELETE FROM webdock_admin.customers WHERE id=ANY($1)", [
        created,
      ]);
      await db.query("DELETE FROM webdock_auth.organization WHERE id=ANY($1)", [
        orgs,
      ]);
      await db.query("DELETE FROM webdock_admin.users WHERE id=$1", [native]);
      await db.query('DELETE FROM webdock_auth."user" WHERE id=ANY($1)', [
        [subject, other],
      ]);
      await registry.close();
      await (await import("../src/modules/registry/context")).closeRegistry();
      await (await import("../src/db/runtime")).closeDatabases();
      await db.end();
    }
  },
  45_000,
);

(enabled ? test : test.skip)("direct Clerk OAuth registry calls obtain bound provider proof without a parent scope", async () => {
  const ownerURL = new URL(process.env.API_TEST_OWNER_URL!);
  if(ownerURL.hostname!=="127.0.0.1" || ownerURL.pathname!=="/webdock_rls_test") throw Error("Owned fixture required");
  const db=new Pool({connectionString:ownerURL.href,max:1});
  const subject="93"+BigInt("0x"+randomBytes(5).toString("hex")).toString();
  const session=subject+"1", native=subject+"2", customer=subject+"3", issuer="https://fixture.clerk.accounts.dev", providerUser="user_"+subject;
  try {
    await db.query(`INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,'Clerk registry fixture',$2,'user',true,false,false)`,[subject,subject+"@example.invalid"]);
    await db.query(`INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval '1 hour',now())`,[session,subject]);
    await db.query("INSERT INTO webdock_auth.clerk_identity(issuer,clerk_user_id,user_id) VALUES($1,$2,$3)",[issuer,providerUser,subject]);
    await db.query("INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,'Clerk registry fixture',$2,'editor',$3)",[native,native+"@example.invalid",subject]);
    await db.query("INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,'Clerk registry fixture','active')",[customer]);
    const script=`
      import {mock} from "bun:test";
      import assert from "node:assert/strict";
      import {currentDatabaseScope} from "@webdock/database-runtime";
      const actor=${JSON.stringify({subject,sessionID:session})};
      let operator=true, reads=0;
      mock.module("./src/modules/auth/clerk-provider",()=>({legacyClerkMemberships:async subject=>{reads++;assert.equal(subject,actor.subject);return {operator,twoFactorEnabled:true,subject,issuer:${JSON.stringify(issuer)},userID:${JSON.stringify(providerUser)},verifiedAt:Date.now(),expiresAt:Date.now()+59000,roles:[]};}}));
      const {createRegistry}=await import("./src/modules/registry/service");
      const registry=await createRegistry();
      try {
        assert.equal(currentDatabaseScope(),undefined);
        const record=await registry.getRecord(actor,"customers",${JSON.stringify(customer)});
        assert.equal(record.id,${JSON.stringify(customer)});
        assert.equal((await registry.operator(actor)).role,"operator");
        assert.equal(currentDatabaseScope(),undefined);
        operator=false;
        await assert.rejects(registry.getRecord(actor,"customers",${JSON.stringify(customer)}),/operator/i);
        operator=true;
        await assert.rejects(registry.getRecord({...actor,sessionID:"wrong-session"},"customers",${JSON.stringify(customer)}),/operator/i);
        assert.equal(reads,4);
      }finally{await registry.close();}
    `;
    const child=Bun.spawnSync([process.execPath,"--no-env-file","-e",script],{cwd:new URL("../",import.meta.url).pathname,env:{...process.env,WEBDOCK_AUTH_PROVIDER:JSON.stringify({provider:"clerk",issuer,authorizedParties:["https://console.example.invalid"],tokenMode:"session"})},stdout:"pipe",stderr:"pipe"});
    expect(child.exitCode,new TextDecoder().decode(child.stderr)).toBe(0);
  }finally{
    await db.query("DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",[customer]);
    await db.query("DELETE FROM webdock_admin.customers WHERE id=$1",[customer]);
    await db.query("DELETE FROM webdock_admin.users WHERE id=$1",[native]);
    await db.query("DELETE FROM webdock_auth.clerk_identity WHERE user_id=$1",[subject]);
    await db.query("DELETE FROM webdock_auth.session WHERE id=$1",[session]);
    await db.query('DELETE FROM webdock_auth."user" WHERE id=$1',[subject]);
    await db.end();
  }
});
