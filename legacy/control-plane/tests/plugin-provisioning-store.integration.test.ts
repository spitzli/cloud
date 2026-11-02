import { pluginScopeSQL } from "../src/modules/plugins/scope-rls";
import { pluginTemplateSchemaSQL } from "../src/modules/hosting/plugin-template-schema";
import { authorizePluginHTTPAction } from "../src/modules/plugins/website-http";
import { authorizePluginAction } from "@webdock/core/plugin-runtime-policy";
import { test, expect } from "bun:test";
import { Pool } from "pg";
import { randomBytes, randomUUID } from "node:crypto";
import { execFileSync } from "node:child_process";
import { createProvisioningStore } from "../src/modules/hosting/plugin-provisioning-store";
import { pluginProvisioningSchemaSQL } from "../../../modules/hosting/src/server/plugin-provisioning-schema";
import {
  pluginProvisioningRlsSQL,
  pluginProvisioningPolicy,
} from "../src/db/plugin-provisioning-rls";
const enabled = process.env.WEBDOCK_TEST_PLUGIN_PROVISIONING_DB === "true";
(enabled ? test : test.skip)(
  "Postgres provisioning serializes duplicates and enforces scoped RLS and live revocation",
  async () => {
    const name = "webdock-provision-test-" + randomBytes(6).toString("hex"),
      password = randomBytes(24).toString("hex");
    const docker = (...args: string[]) =>
      execFileSync("docker", args, { encoding: "utf8" }).trim();
    docker(
      "run",
      "-d",
      "--name",
      name,
      "-e",
      "POSTGRES_PASSWORD=" + password,
      "-p",
      "127.0.0.1::5432",
      "postgres:18-alpine",
    );
    let pool: Pool | undefined;
    let closeRuntime: (() => Promise<void>) | undefined;
    const originalEnvironment = { ...process.env };
    const invocation = randomUUID();
    try {
      const port = docker("port", name, "5432/tcp").split(":").at(-1);
      pool = new Pool({
        connectionString: `postgresql://postgres:${password}@127.0.0.1:${port}/postgres`,
        max: 16,
      });
      for (let i = 0; ; i++) {
        try {
          await pool.query("SELECT 1");
          break;
        } catch (e) {
          if (i > 80) throw e;
          await new Promise((r) => setTimeout(r, 100));
        }
      }
      await pool.query(`CREATE SCHEMA webdock_auth;CREATE SCHEMA webdock_admin;CREATE SCHEMA webdock_rls;
   CREATE ROLE webdock_platform_owner BYPASSRLS;CREATE ROLE webdock_auth_runtime;CREATE ROLE webdock_worker_runtime;
   GRANT USAGE ON SCHEMA webdock_auth,webdock_admin,webdock_rls TO webdock_platform_owner,webdock_auth_runtime,webdock_worker_runtime;
   CREATE SEQUENCE webdock_auth.ids START 100;
   CREATE FUNCTION webdock_auth.next_snowflake() RETURNS varchar LANGUAGE sql AS $$ SELECT nextval('webdock_auth.ids')::text $$;
   GRANT USAGE ON SEQUENCE webdock_auth.ids TO webdock_auth_runtime;
   CREATE TABLE webdock_admin.customers(id varchar PRIMARY KEY,status text);INSERT INTO webdock_admin.customers VALUES('20','active');
   CREATE TABLE webdock_admin.projects(id varchar PRIMARY KEY,customer_id varchar,status text,UNIQUE(id,customer_id));INSERT INTO webdock_admin.projects VALUES('10','20','active'),('11','20','active');
   CREATE TABLE webdock_admin.project_deletion(project_id varchar,status text);
   CREATE TABLE webdock_auth.hosting_operation(id varchar,project_id varchar,idempotency_key text,result jsonb);
   CREATE TABLE webdock_auth.hosting_setup(id varchar PRIMARY KEY,project_id varchar,customer_id varchar,state text,resources jsonb DEFAULT '{}',steps jsonb DEFAULT '[]');INSERT INTO webdock_auth.hosting_setup(id,project_id,customer_id,state) VALUES('500','10','20','running');
   CREATE TABLE webdock_auth."user"(id text,name text,email text,role text,banned boolean,"emailVerified" boolean,"mustChangePassword" boolean,"twoFactorEnabled" boolean);INSERT INTO webdock_auth."user" VALUES('user','User','user@example.test','user',false,true,false,false);
   CREATE TABLE webdock_auth.tenant_customer(customer_id text,organization_id text);INSERT INTO webdock_auth.tenant_customer VALUES('20','org');
   CREATE TABLE webdock_auth.hosting_project(project_id text,mode text,cluster_id text,namespace text);INSERT INTO webdock_auth.hosting_project VALUES('10','selfservice','30','wd-10');
   CREATE TABLE webdock_auth.hosting_app(id text,project_id text,revision int,status text);
   CREATE TABLE webdock_auth.session(id text,"userId" text,"expiresAt" timestamptz);INSERT INTO webdock_auth.session VALUES('session','user',now()+interval '1 hour');
   CREATE TABLE webdock_auth.member("userId" text,"organizationId" text,role text);INSERT INTO webdock_auth.member VALUES('user','org','admin');
   CREATE TABLE webdock_auth.studio_tenant_preview(session_id text);
   CREATE TABLE webdock_auth.console_tenant_preview(session_id text,actor_id text,issuer text);
   CREATE TABLE webdock_auth.clerk_identity(user_id text,issuer text,clerk_user_id text,enabled boolean);
   CREATE TABLE webdock_auth.clerk_organization(issuer text,clerk_organization_id text,organization_id text,enabled boolean);
   CREATE TABLE webdock_auth.plugin_installation(id text PRIMARY KEY,state text,desired_state text,runtime text,current_generation int);INSERT INTO webdock_auth.plugin_installation VALUES('example','enabled','enabled','ready',2);
   CREATE TABLE webdock_auth.plugin_runtime(plugin_id text,generation int,status text,configuration_revision int);INSERT INTO webdock_auth.plugin_runtime VALUES('example',2,'ready',1);
   CREATE TABLE webdock_auth.plugin_configuration(plugin_id text,revision int,approved_capabilities jsonb);INSERT INTO webdock_auth.plugin_configuration VALUES('example',1,'["plugin.apps.provision","plugin.apps.read","plugin.http.read","plugin.http.request"]');
   CREATE TABLE webdock_auth.plugin_grant(plugin_id text,project_id varchar,customer_id varchar,revision int,revoked_at timestamptz,capabilities jsonb);INSERT INTO webdock_auth.plugin_grant VALUES('example','10','20',3,NULL,'["plugin.apps.provision","plugin.apps.read","plugin.http.read","plugin.http.request"]');
   CREATE FUNCTION webdock_rls.request_scope() RETURNS jsonb LANGUAGE sql AS $$ SELECT coalesce(nullif(current_setting('test.scope',true),''),'{}')::jsonb $$;
   CREATE FUNCTION webdock_rls.worker(purposes text[]) RETURNS bool LANGUAGE sql AS $$ SELECT coalesce(webdock_rls.request_scope()->>'purpose'=ANY(purposes),false) $$;
   CREATE FUNCTION webdock_rls.agent_project(project text) RETURNS bool LANGUAGE sql AS $$ SELECT webdock_rls.worker(ARRAY['agent']) AND webdock_rls.request_scope()->>'projectID'=project $$;
   CREATE FUNCTION webdock_rls.setup_allowed(customer text,project text) RETURNS bool LANGUAGE sql AS $$ SELECT webdock_rls.worker(ARRAY['hosting-setup']) AND webdock_rls.request_scope()->>'customerID'=customer $$;
   CREATE TABLE webdock_auth.plugin_invocation(id uuid,plugin_id text,customer_id text,project_id text,runtime_generation int,grant_revision int,kind text,name text,intent text,actor_id text,session_id text,operation_id text,capabilities jsonb,created_at timestamptz DEFAULT now(),expires_at timestamptz DEFAULT now()+interval '59 seconds',revoked_at timestamptz);
   INSERT INTO webdock_auth.plugin_invocation(id,plugin_id,customer_id,project_id,runtime_generation,grant_revision,kind,name,intent,actor_id,session_id,operation_id,capabilities) VALUES('${invocation}','example','20','10',2,3,'action','provision','write','user','session','operation','["plugin.apps.provision","plugin.apps.read","plugin.http.read","plugin.http.request"]');
   CREATE FUNCTION webdock_rls.actor() RETURNS jsonb LANGUAGE sql AS $$ SELECT jsonb_build_object('subject',webdock_rls.request_scope()->>'subject','sessionID',webdock_rls.request_scope()->>'sessionID','preview',NULL) $$;
   CREATE FUNCTION webdock_rls.customer_allowed(customer text,write boolean DEFAULT false) RETURNS boolean LANGUAGE sql AS $$ SELECT webdock_rls.request_scope()->>'customerID'=customer $$;
   CREATE TABLE webdock_auth."oauthClient"("clientId" text,disabled boolean,"redirectUris" jsonb);
   CREATE TABLE webdock_auth.app_binding(id text,client_id text,enabled boolean,organization_id text);
   CREATE TABLE webdock_auth.project_grant(id text,user_id text,binding_id text,organization_id text,enabled boolean,role text);
   CREATE TABLE webdock_auth.platform_settings(id boolean,settings jsonb);
   INSERT INTO webdock_auth."oauthClient" VALUES('client',false,'["https://cms.example.test/api/sso/callback"]');
   INSERT INTO webdock_auth.app_binding VALUES('123','client',true,'org');
   INSERT INTO webdock_auth.project_grant VALUES('grant','user','123','org',true,'reader');
`);
      await pool.query(pluginProvisioningSchemaSQL);
      await pool.query(
        "GRANT ALL ON ALL TABLES IN SCHEMA webdock_auth,webdock_admin TO webdock_platform_owner;GRANT SELECT ON webdock_auth.hosting_setup TO webdock_auth_runtime",
      );
      await pool.query(pluginScopeSQL);
      await pool.query(pluginProvisioningRlsSQL);
      await pool.query(pluginTemplateSchemaSQL);
      for (const table of [
        "hosting_plugin_provisioning",
        "hosting_plugin_provisioning_request",
      ]) {
        const p = pluginProvisioningPolicy(table);
        await pool.query(
          `ALTER TABLE webdock_auth.${table} ENABLE ROW LEVEL SECURITY;ALTER TABLE webdock_auth.${table} FORCE ROW LEVEL SECURITY;GRANT SELECT,INSERT,UPDATE ON webdock_auth.${table} TO webdock_auth_runtime,webdock_worker_runtime;CREATE POLICY runtime_read ON webdock_auth.${table} FOR SELECT TO webdock_auth_runtime USING(${p.read});CREATE POLICY runtime_insert ON webdock_auth.${table} FOR INSERT TO webdock_auth_runtime WITH CHECK(${p.write});CREATE POLICY runtime_update ON webdock_auth.${table} FOR UPDATE TO webdock_auth_runtime USING(${p.write}) WITH CHECK(${p.write});CREATE POLICY worker_read ON webdock_auth.${table} FOR SELECT TO webdock_worker_runtime USING(${p.workerRead});CREATE POLICY worker_update ON webdock_auth.${table} FOR UPDATE TO webdock_worker_runtime USING(${p.workerWrite}) WITH CHECK(${p.workerWrite});`,
        );
      }
      const scope = {
        pluginInvocationID: invocation,
        pluginID: "example",
        customerID: "20",
        projectID: "10",
        subject: "user",
        sessionID: "session",
        operationID: "operation",
        pluginGeneration: 2,
        grantRevision: 3,
      };
      const scoped = (
        value: Record<string, unknown>,
        role = "webdock_auth_runtime",
      ) =>
        createProvisioningStore(async (run) => {
          const db = await pool!.connect();
          try {
            await db.query("BEGIN");
            await db.query(`SET LOCAL ROLE ${role}`);
            await db.query("SELECT set_config('test.scope',$1,true)", [
              JSON.stringify(value),
            ]);
            const result = await run(db);
            await db.query("COMMIT");
            return result;
          } catch (e) {
            await db.query("ROLLBACK");
            throw e;
          } finally {
            db.release();
          }
        });
      const store = scoped(scope);
      await store.assertLive(scope);
      const records = await Promise.all(
        Array.from({ length: 12 }, (_, i) =>
          store.reserve(scope, {
            template_id: "approved",
            name: "App",
            request_id: "request-" + i,
          }),
        ),
      );
      expect(new Set(records.map((r) => r.id)).size).toBe(1);
      expect(
        (
          await pool.query(
            "SELECT count(*)::int AS count FROM webdock_auth.hosting_plugin_provisioning",
          )
        ).rows[0].count,
      ).toBe(1);
      await expect(
        store.reserve(scope, {
          template_id: "other",
          name: "App",
          request_id: "request-1",
        }),
      ).rejects.toMatchObject({ code: "conflict" });
      await store.attachSetup(records[0], "500");
      expect(
        await scoped({ ...scope, projectID: "11" }).get(
          { ...scope, projectID: "11" },
          records[0].id,
        ),
      ).toBeNull();
      const readyBinding = {
        appID: "600",
        websiteBindingID: "123",
        origin: "https://cms.example.test",
        secretReference: "opaque",
        pinnedAddresses: ["203.0.113.10"],
      };
      await store.setBinding(records[0], readyBinding);
      await pool.query("UPDATE webdock_auth.hosting_setup SET state='ready'");
      await pool.query(
        "UPDATE webdock_auth.plugin_invocation SET capabilities='[\"plugin.http.read\"]',intent='read'",
      );
      expect(await store.resolve(scope, "approved")).toEqual(readyBinding);
      expect(
        await store.resolve({ ...scope, pluginID: "other" }, "approved"),
      ).toBeNull();
      expect(
        await store.resolve({ ...scope, projectID: "11" }, "approved"),
      ).toBeNull();
      await expect(store.assertLive(scope)).rejects.toMatchObject({
        code: "permission_denied",
      });
      const url = `postgresql://postgres:${password}@127.0.0.1:${port}/postgres`;
      Object.assign(process.env, {
        DATABASE_URL: url,
        AUTH_DATABASE_URL: url,
        DATABASE_WORKER_URL: url,
        NODE_ENV: "development",
        WEBDOCK_AUTH_PROVIDER: '{"provider":"better-auth"}',
        WEBDOCK_PROCESS_ROLE: "api",
      });
      delete process.env.DATABASE_RLS_SECRET;
      const { currentClaims } =
        await import("../src/modules/auth/authorization");
      closeRuntime = (await import("../src/db/runtime")).closeDatabases;
      await pool.query("UPDATE webdock_auth.member SET role='member'");
      const websiteScope = {
        subject: "user",
        customerID: "20",
        projectID: "10",
        generation: 2,
        grantRevision: 3,
      };
      const project = {
        active: true,
        allowed: true,
        preview: false,
        canWrite: false,
      };
      const readAction: any = {
        name: "cms.read",
        intent: "read",
        httpBinding: "cms",
        capabilities: ["plugin.http.read"],
      };
      const writeAction: any = {
        name: "cms.write",
        intent: "write",
        httpBinding: "cms",
        capabilities: ["plugin.http.request"],
      };
      const authority = {
        claims: currentClaims,
        website: async (id: string) =>
          (
            await pool!.query(
              'SELECT b.enabled AND NOT c.disabled AS enabled,t.customer_id AS "customerID",c."redirectUris" AS "redirectURIs" FROM webdock_auth.app_binding b JOIN webdock_auth."oauthClient" c ON c."clientId"=b.client_id JOIN webdock_auth.tenant_customer t ON t.organization_id=b.organization_id WHERE b.id=$1',
              [id],
            )
          ).rows[0],
        resolve: async (alias: string) => {
          const b =
            alias === "cms" ? await store.resolve(scope, "approved") : null;
          return b
            ? {
                ...b,
                addresses: b.pinnedAddresses,
                headers: { Authorization: "Bearer fixture" },
                rules: [
                  {
                    method: "GET" as const,
                    path: "/api/webdock/cms",
                    query: [],
                  },
                ],
              }
            : null;
        },
      };
      expect(
        (
          await authorizePluginHTTPAction(
            websiteScope,
            readAction,
            project,
            authority,
          )
        )?.headers["x-webdock-role"],
      ).toBe("reader");
      await expect(
        authorizePluginHTTPAction(
          websiteScope,
          writeAction,
          project,
          authority,
        ),
      ).rejects.toMatchObject({ code: "permission_denied" });
      await pool.query("UPDATE webdock_auth.project_grant SET role='editor'");
      await pool.query(
        "UPDATE webdock_auth.plugin_invocation SET capabilities='[\"plugin.http.request\"]',intent='write'",
      );
      expect(
        (
          await authorizePluginHTTPAction(
            websiteScope,
            writeAction,
            project,
            authority,
          )
        )?.headers["x-webdock-role"],
      ).toBe("editor");
      await expect(
        authorizePluginHTTPAction(
          websiteScope,
          writeAction,
          project,
          authority,
          { http: { name: "other", method: "POST" } },
        ),
      ).rejects.toMatchObject({ code: "permission_denied" });
      const adminPolicy: any = {
        state: "enabled",
        runtime: "ready",
        generation: 2,
        runtimeGeneration: 2,
        configurationRevision: 1,
        runtimeConfigurationRevision: 1,
        grantRevision: 3,
        revoked: false,
        approved: ["plugin.apps.provision"],
        granted: ["plugin.apps.provision"],
        actions: [
          {
            name: "provision",
            intent: "write",
            capabilities: ["plugin.apps.provision"],
          },
        ],
        project: { ...project, id: "10", customerID: "20" },
      };
      expect(() =>
        authorizePluginAction(adminPolicy, "provision", {
          deferWebsiteHTTP: true,
        }),
      ).toThrow();
      await pool.query("UPDATE webdock_auth.project_grant SET enabled=false");
      await expect(
        authorizePluginHTTPAction(
          websiteScope,
          writeAction,
          project,
          authority,
        ),
      ).rejects.toMatchObject({ code: "permission_denied" });
      await pool.query("UPDATE webdock_auth.member SET role='admin'");
      expect(
        authorizePluginAction(
          {
            ...adminPolicy,
            project: { ...adminPolicy.project, canWrite: true },
          },
          "provision",
        ).name,
      ).toBe("provision");
      await pool.query(
        "UPDATE webdock_auth.plugin_invocation SET capabilities='[\"plugin.apps.provision\",\"plugin.apps.read\"]',intent='write'",
      );
      await pool.query("UPDATE webdock_auth.hosting_setup SET state='running'");
      const worker = scoped(
        { purpose: "hosting-setup", setupID: "500", customerID: "20" },
        "webdock_worker_runtime",
      );
      expect((await worker.bySetup("500"))?.id).toBe(records[0].id);
      await worker.assertLive(scope);
      expect(await store.status(records[0])).toEqual({
        state: "running",
        resources: {},
        steps: [],
      });
      await pool.query(
        "INSERT INTO webdock_auth.hosting_operation VALUES('700','10','setup-500-release',NULL),('701','10','database-job-random',NULL),('702','10','setup-500-database','{\"operationID\":\"701\"}')",
      );
      const agent = scoped(
        { purpose: "agent", projectID: "10" },
        "webdock_worker_runtime",
      );
      await agent.assertLive(scope);
      const actorConnection = await pool.connect();
      try {
        await actorConnection.query("BEGIN");
        await actorConnection.query("SET LOCAL ROLE webdock_worker_runtime");
        await actorConnection.query("SELECT set_config('test.scope',$1,true)", [
          JSON.stringify({ purpose: "agent", projectID: "10" }),
        ]);
        const allowedActor = (
          await actorConnection.query(
            "SELECT webdock_rls.plugin_provisioning_actor($1) AS actor",
            [records[0].id],
          )
        ).rows[0].actor;
        expect(allowedActor.legacyAllowed).toBe(true);
        await pool.query("UPDATE webdock_auth.member SET role='member'");
        const revokedActor = (
          await actorConnection.query(
            "SELECT webdock_rls.plugin_provisioning_actor($1) AS actor",
            [records[0].id],
          )
        ).rows[0].actor;
        expect(revokedActor.legacyAllowed).toBe(false);
        await actorConnection.query("ROLLBACK");
      } finally {
        actorConnection.release();
      }

      await pool.query("UPDATE webdock_auth.plugin_grant SET revoked_at=now()");
      await expect(worker.assertLive(scope)).rejects.toMatchObject({
        code: "permission_denied",
      });
      expect((await worker.bySetup("500"))?.id).toBe(records[0].id);
      const guarded = await pool.query(
        "SELECT webdock_rls.plugin_setup_operation_binding('700') AS app,webdock_rls.plugin_setup_operation_binding('701') AS db,webdock_rls.plugin_setup_operation_live('700') AS allowed",
      );
      expect(guarded.rows[0]).toEqual({
        app: records[0].id,
        db: records[0].id,
        allowed: false,
      });
      await pool.query(
        'UPDATE webdock_auth.hosting_plugin_provisioning SET template_state=\'{"migration":{"status":"queued","leaseToken":"private"},"sentinel":true}\' WHERE id=$1',
        [records[0].id],
      );
      const repair = await pool.connect();
      try {
        await repair.query("BEGIN");
        await repair.query("SET LOCAL ROLE webdock_worker_runtime");
        await repair.query("SELECT set_config('test.scope',$1,true)", [
          JSON.stringify({ purpose: "agent", projectID: "10" }),
        ]);
        expect(
          (
            await repair.query(
              "SELECT webdock_auth.block_template_operation($1,'migration','authority_changed') AS blocked",
              [records[0].id],
            )
          ).rows[0].blocked,
        ).toBe(true);
        await repair.query("COMMIT");
        const retained = (
          await pool.query(
            "SELECT template_state,binding FROM webdock_auth.hosting_plugin_provisioning WHERE id=$1",
            [records[0].id],
          )
        ).rows[0];
        expect(retained.template_state).toEqual({
          migration: {
            status: "needs-reconciliation",
            reason: "authority_changed",
          },
          sentinel: true,
        });
        expect(retained.binding).toEqual(readyBinding);
        await pool.query(
          "UPDATE webdock_auth.hosting_plugin_provisioning SET template_state=jsonb_build_object('migration',jsonb_build_object('status','running','leaseToken','private','leaseUntil',now()+interval '1 hour')) WHERE id=$1",
          [records[0].id],
        );
        await repair.query("BEGIN");
        await repair.query("SET LOCAL ROLE webdock_worker_runtime");
        await repair.query("SELECT set_config('test.scope',$1,true)", [
          JSON.stringify({ purpose: "agent", projectID: "10" }),
        ]);
        expect(
          (
            await repair.query(
              "SELECT webdock_auth.block_template_operation($1,'migration','authority_changed') AS blocked",
              [records[0].id],
            )
          ).rows[0].blocked,
        ).toBe(false);
        await repair.query("SELECT set_config('test.scope',$1,true)", [
          JSON.stringify({ purpose: "agent", projectID: "11" }),
        ]);
        await expect(
          repair.query(
            "SELECT webdock_auth.block_template_operation($1,'migration','authority_changed')",
            [records[0].id],
          ),
        ).rejects.toMatchObject({ code: "42501" });
        await repair.query("ROLLBACK");
        await pool.query(
          "UPDATE webdock_auth.hosting_plugin_provisioning SET template_state=jsonb_set(template_state,'{migration,leaseUntil}',to_jsonb((now()-interval '1 second')::text)) WHERE id=$1",
          [records[0].id],
        );
        await repair.query("BEGIN");
        await repair.query("SET LOCAL ROLE webdock_worker_runtime");
        await repair.query("SELECT set_config('test.scope',$1,true)", [
          JSON.stringify({ purpose: "agent", projectID: "10" }),
        ]);
        expect(
          (
            await repair.query(
              "SELECT webdock_auth.block_template_operation($1,'migration','authority_changed') AS blocked",
              [records[0].id],
            )
          ).rows[0].blocked,
        ).toBe(true);
        await repair.query("COMMIT");
      } finally {
        repair.release();
      }
      const { assertTemplateApproval } =
        await import("../src/modules/hosting/plugin-template-runtime");
      const approvedConfig = {
        image: "ghcr.io/example/cms@sha256:" + "a".repeat(64),
        domain: "cms.example.com",
        addresses: ["192.0.2.1", "192.0.2.2"],
        certificateIssuer: "cms-acme",
        ingressClass: "traefik",
        planRevisionID: "123",
        authIssuer: "https://auth.example.com/api/auth",
        authAddresses: ["192.0.2.3"],
      };
      const storedConfig = (
        await pool.query("SELECT $1::jsonb AS approval", [
          JSON.stringify(approvedConfig),
        ])
      ).rows[0].approval;
      expect(Object.keys(storedConfig)).not.toEqual(
        Object.keys(approvedConfig),
      );
      expect(() =>
        assertTemplateApproval(storedConfig, approvedConfig),
      ).not.toThrow();
      expect(() =>
        assertTemplateApproval(
          { ...storedConfig, addresses: [...storedConfig.addresses].reverse() },
          approvedConfig,
        ),
      ).toThrow();
      // Exercise the production claim query against PostgreSQL: twenty old pending
      // certificates must not exclude a newer instance indefinitely.
      const { templateClaimSQL } =
        await import("../src/modules/hosting/plugin-template-runtime");
      await pool.query(`ALTER TABLE webdock_auth.hosting_setup ADD COLUMN configuration jsonb DEFAULT '{}', ADD COLUMN preview jsonb;
        ALTER TABLE webdock_auth.hosting_app ADD COLUMN spec jsonb DEFAULT '{}', ADD COLUMN lifecycle text DEFAULT 'draft', ADD COLUMN observed_uid text;
        INSERT INTO webdock_admin.projects(id,customer_id,status) SELECT (2000+i)::text,'20','active' FROM generate_series(1,21)i;
        INSERT INTO webdock_auth.hosting_project(project_id,mode,cluster_id,namespace) SELECT (2000+i)::text,'selfservice','99','wd-'||(2000+i)::text FROM generate_series(1,21)i;
        INSERT INTO webdock_auth.hosting_app(id,project_id,revision,status) SELECT (3000+i)::text,(2000+i)::text,1,'ready' FROM generate_series(1,21)i;
        INSERT INTO webdock_auth.hosting_setup(id,project_id,customer_id,state,resources) SELECT (4000+i)::text,(2000+i)::text,'20','running',jsonb_build_object('appID',(3000+i)::text) FROM generate_series(1,21)i;
        INSERT INTO webdock_auth.hosting_plugin_provisioning(id,plugin_id,customer_id,project_id,template_id,name,actor,plugin_generation,grant_revision,setup_id,template_state,created_at,updated_at)
          SELECT (5000+i)::text,'queue-test','20',(2000+i)::text,'managed-cms','Pending','{}',1,1,(4000+i)::text,
          '{"migration":{"status":"succeeded"},"publication":{"status":"queued"}}','2000-01-01'::timestamptz+i*interval '1 second','2000-01-01'::timestamptz+i*interval '1 second' FROM generate_series(1,21)i;`);
      const selected: string[] = [];
      for (let attempt = 0; attempt < 21; attempt++) {
        const candidate = (await pool.query(templateClaimSQL, ["99"])).rows[0];
        selected.push(candidate.id);
        await pool.query(
          "UPDATE webdock_auth.hosting_plugin_provisioning SET updated_at=clock_timestamp() WHERE id=$1",
          [candidate.id],
        );
      }
      expect(new Set(selected).size).toBe(21);
      expect(selected).toContain("5021");
      await pool.query(
        "UPDATE webdock_auth.hosting_plugin_provisioning SET template_state=jsonb_set(template_state,'{publication}',jsonb_build_object('status','running','leaseUntil',now()+interval '1 hour')),updated_at='2000-01-01' WHERE plugin_id='queue-test' AND id<>'5021'",
      );
      expect(
        (await pool.query(templateClaimSQL, ["99"])).rows.map((row) => row.id),
      ).toEqual(["5021"]);
    } finally {
      await closeRuntime?.();
      for (const key of [
        "DATABASE_URL",
        "AUTH_DATABASE_URL",
        "DATABASE_WORKER_URL",
        "NODE_ENV",
        "WEBDOCK_AUTH_PROVIDER",
        "WEBDOCK_PROCESS_ROLE",
        "DATABASE_RLS_SECRET",
      ]) {
        if (originalEnvironment[key] === undefined) delete process.env[key];
        else process.env[key] = originalEnvironment[key];
      }
      await pool?.end();
      docker("rm", "-f", name);
    }
  },
  20000,
);
