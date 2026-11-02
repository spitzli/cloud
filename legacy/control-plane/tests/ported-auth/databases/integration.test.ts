import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { Pool } from "pg";
import { databaseSchemaSQL } from "../../../src/modules/databases/schema";
import {
  executeDatabase,
  exchangeLaunch,
  inspectSession,
} from "../../../src/modules/databases/service";
import { encryptMailSecret } from "../../../src/modules/platform/platform";
import type { HostingActor } from "@webdock/hosting-contracts";

const url = process.env.WEBDOCK_DATABASE_TEST_URL;
test(
  "database grants, launches and revocation use real tenant boundaries",
  { skip: !url },
  async () => {
    const target = new URL(url!);
    assert.ok(
      ["localhost", "127.0.0.1"].includes(target.hostname) &&
        target.pathname === "/webdock_api_test" &&
        target.port === "55441",
      "Use the dedicated disposable database-browser fixture",
    );
    const pool = new Pool({ connectionString: url });
    const db = await pool.connect();
    try {
      await db.query(`
      DROP SCHEMA IF EXISTS webdock_auth CASCADE; DROP SCHEMA IF EXISTS webdock_admin CASCADE;
      CREATE SCHEMA webdock_auth; CREATE SCHEMA webdock_admin;
      CREATE SEQUENCE webdock_auth.ids START 10000;
      CREATE FUNCTION webdock_auth.next_snowflake() RETURNS text LANGUAGE sql AS $$ SELECT nextval('webdock_auth.ids')::text $$;
      CREATE TABLE webdock_auth."user"(id text PRIMARY KEY,role text,banned boolean DEFAULT false,"emailVerified" boolean DEFAULT true,"mustChangePassword" boolean DEFAULT false,"twoFactorEnabled" boolean DEFAULT false);
      CREATE TABLE webdock_auth.session(id text PRIMARY KEY,"userId" text,"expiresAt" timestamptz);
      CREATE TABLE webdock_admin.customers(id varchar PRIMARY KEY,status text DEFAULT 'active');
      CREATE TABLE webdock_admin.projects(id varchar PRIMARY KEY,customer_id varchar REFERENCES webdock_admin.customers(id),status text DEFAULT 'active',name text);
      CREATE TABLE webdock_auth.tenant_customer(customer_id varchar,organization_id text);
      CREATE TABLE webdock_auth.member("organizationId" text,"userId" text,role text);
      CREATE TABLE webdock_auth.hosting_project(project_id varchar,mode text,provider text,cluster_id varchar,revision integer,own_images boolean);
      CREATE TABLE webdock_auth.hosting_database_backend(id varchar PRIMARY KEY,host text,direct_port integer,endpoint_ready boolean,ca_certificate text,state text);
      CREATE TABLE webdock_auth.hosting_database(id varchar PRIMARY KEY,project_id varchar,customer_id varchar,name text,database_name text,backend_id varchar,revision integer DEFAULT 1,state text DEFAULT 'ready',credential_revision integer DEFAULT 1,confirmed_credential_revision integer DEFAULT 1,confirmed_credentials_encrypted text,pending_backend_id varchar);
      CREATE TABLE webdock_auth.hosting_audit(subject text,action text,target_id text,outcome text);
      CREATE TABLE webdock_auth.studio_tenant_preview(session_id text,actor_id text,customer_id varchar,organization_id text,expires_at timestamptz,expired_at timestamptz);
      CREATE FUNCTION webdock_admin.lock_hosting_project(varchar) RETURNS void LANGUAGE plpgsql AS $$ BEGIN PERFORM id FROM webdock_admin.projects WHERE id=$1 FOR SHARE; END $$;
      INSERT INTO webdock_auth."user"(id,role,"twoFactorEnabled") VALUES('operator','operator',true),('alice','user',false),('bob','user',false),('eve','user',false);
      INSERT INTO webdock_auth.session SELECT id,id,now()+interval '1 hour' FROM webdock_auth."user";
      INSERT INTO webdock_admin.customers(id) VALUES('100'),('200');
      INSERT INTO webdock_admin.projects(id,customer_id,name) VALUES('101','100','A'),('201','200','B');
      INSERT INTO webdock_auth.tenant_customer VALUES('100','org-a'),('200','org-b');
      INSERT INTO webdock_auth.member VALUES('org-a','alice','admin'),('org-a','bob','member'),('org-b','eve','admin');
    `);
      await db.query(
        readFileSync(
          new URL(
            "../../../drizzle/0022_managed_database_viewer.sql",
            import.meta.url,
          ),
          "utf8",
        ),
      );
      await db.query(databaseSchemaSQL);
      await db.query(databaseSchemaSQL);
      const actor = (subject: string): HostingActor => ({
        subject,
        sessionID: subject,
        source: "studio",
        scopes: ["hosting:read", "hosting:write"],
      });
      const call = (subject: string, command: unknown) =>
        executeDatabase(db, actor(subject), command);
      await db.query(
        "INSERT INTO webdock_auth.hosting_database_backend VALUES('301','db.example.invalid',30432,true,'test-ca','ready')",
      );
      await db.query(
        "INSERT INTO webdock_auth.hosting_database(id,project_id,customer_id,name,database_name,backend_id,confirmed_credentials_encrypted) VALUES('401','101','100','Managed Orders','wd_401','301',$1)",
        [
          encryptMailSecret(
            {
              applicationPassword: "app-secret",
              migrationPassword: "migration-secret",
            },
            "database:401:1",
          ),
        ],
      );
      const managedList = (await call("alice", {
        action: "list",
        customerID: "100",
      })) as {
        managed: { id: string; canOpen: boolean }[];
        bindings: unknown[];
      };
      assert.deepEqual(
        managedList.managed.map((d) => [d.id, d.canOpen]),
        [["401", true]],
      );
      assert.deepEqual(managedList.bindings, []);
      const memberList = (await call("bob", {
        action: "list",
        customerID: "100",
      })) as typeof managedList;
      assert.equal(memberList.managed[0].canOpen, false);
      const managedOpen = () =>
        call("alice", { action: "openManaged", databaseID: "401" }) as Promise<{
          code: string;
          binding: { id: string; profile: string };
        }>;
      for (const subject of ["bob", "eve"])
        await assert.rejects(
          call(subject, { action: "openManaged", databaseID: "401" }),
        );
      const managedLaunch = await managedOpen();
      assert.equal(managedLaunch.binding.id, "401");
      assert.equal(managedLaunch.binding.profile, "schema");
      assert.ok(!JSON.stringify(managedLaunch).includes("secret"));
      const managedSession = await exchangeLaunch(db, managedLaunch.code);
      await assert.rejects(exchangeLaunch(db, managedLaunch.code));
      const managedAccess = await inspectSession(db, managedSession.token);
      assert.ok("kind" in managedAccess);
      if (!("kind" in managedAccess) || managedAccess.kind !== "managed")
        throw Error("Expected managed access");
      assert.equal(managedAccess.database.username, "wd_migrate_401");
      assert.equal(managedAccess.database.password, "migration-secret");
      assert.equal(managedAccess.database.caCertificate, "test-ca");
      await db.query(
        "UPDATE webdock_auth.member SET role='member' WHERE \"userId\"='alice'",
      );
      await assert.rejects(inspectSession(db, managedSession.token));
      await db.query(
        "UPDATE webdock_auth.member SET role='admin' WHERE \"userId\"='alice'",
      );
      for (const patch of [
        "state='paused'",
        "credential_revision=2",
        "backend_id='302'",
        "revision=2",
        "pending_backend_id='302'",
      ]) {
        const invalidatedLaunch = await managedOpen();
        await db.query(
          "UPDATE webdock_auth.hosting_database SET " +
            patch +
            " WHERE id='401'",
        );
        await assert.rejects(inspectSession(db, managedSession.token), patch);
        await assert.rejects(exchangeLaunch(db, invalidatedLaunch.code), patch);
        await db.query(
          "UPDATE webdock_auth.hosting_database SET state='ready',credential_revision=1,backend_id='301',revision=1,pending_backend_id=NULL WHERE id='401'",
        );
      }
      await db.query(
        "UPDATE webdock_auth.hosting_database_backend SET endpoint_ready=false WHERE id='301'",
      );
      await assert.rejects(managedOpen());
      await assert.rejects(inspectSession(db, managedSession.token));
      await db.query(
        "UPDATE webdock_auth.hosting_database_backend SET endpoint_ready=true WHERE id='301'",
      );
      const expiredManaged = await managedOpen();
      await db.query(
        "UPDATE webdock_auth.managed_database_launch SET expires_at=now()-interval '1 second'",
      );
      await assert.rejects(exchangeLaunch(db, expiredManaged.code));
      await db.query(
        "INSERT INTO webdock_auth.studio_tenant_preview VALUES('alice','alice','100','org-a',now()+interval '10 minutes',null)",
      );
      await assert.rejects(managedOpen());
      await assert.rejects(inspectSession(db, managedSession.token));
      await db.query("DELETE FROM webdock_auth.studio_tenant_preview");
      const operatorLaunch = (await call("operator", {
        action: "openManaged",
        databaseID: "401",
      })) as { code: string };
      const operatorSession = await exchangeLaunch(db, operatorLaunch.code);
      await db.query(
        `UPDATE webdock_auth."user" SET "twoFactorEnabled"=false WHERE id='operator'`,
      );
      await assert.rejects(inspectSession(db, operatorSession.token));
      await assert.rejects(
        call("operator", { action: "openManaged", databaseID: "401" }),
      );
      await db.query(
        `UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id='operator'`,
      );
      const revokedLaunch = await managedOpen();
      await db.query(
        `UPDATE webdock_auth.session SET "expiresAt"=now()-interval '1 second' WHERE id='alice'`,
      );
      await assert.rejects(exchangeLaunch(db, revokedLaunch.code));
      await assert.rejects(inspectSession(db, managedSession.token));
      await db.query(
        `UPDATE webdock_auth.session SET "expiresAt"=now()+interval '1 hour' WHERE id='alice'`,
      );
      await db.query(
        "UPDATE webdock_auth.managed_database_session SET expires_at=now()-interval '1 second'",
      );
      await assert.rejects(inspectSession(db, managedSession.token));
      await db.query(
        "UPDATE webdock_auth.hosting_database SET state='purged' WHERE id='401'",
      );
      assert.deepEqual(
        (
          (await call("alice", {
            action: "list",
            customerID: "100",
          })) as typeof managedList
        ).managed,
        [],
      );
      const created = (await call("operator", {
        action: "create",
        projectID: "101",
        name: "Orders",
        environment: "production",
      })) as { id: string };
      const bindingID = created.id;
      const grant = {
        action: "grant",
        bindingID,
        subject: "alice",
        profile: "read",
        runtimeOrigin: "https://runtime-a.example.invalid",
        connectionID: "orders",
        proxySecret: "a".repeat(40),
        isolationVerified: true,
        databaseRoleVerified: true,
      };
      await assert.rejects(call("alice", grant));
      await assert.rejects(call("operator", { ...grant, subject: "eve" }));
      await call("operator", grant);
      await db.query(
        'UPDATE webdock_auth."user" SET "twoFactorEnabled"=false WHERE id=\'operator\'',
      );
      await assert.rejects(call("operator", grant));
      await db.query(
        'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=\'operator\'',
      );
      await assert.rejects(
        call("operator", { ...grant, profile: "write" }),
        /isolated runtime/i,
      );
      await call("operator", {
        ...grant,
        runtimeOrigin: "https://runtime-new.example.invalid",
      });
      await assert.rejects(
        call("operator", { ...grant, subject: "bob" }),
        /runtime/i,
      );
      await call("operator", grant);
      const list = (await call("alice", {
        action: "list",
        customerID: "100",
      })) as { bindings: unknown[] };
      assert.equal(list.bindings.length, 1);
      assert.ok(!JSON.stringify(list).includes("runtime-a"));
      await assert.rejects(call("eve", { action: "get", bindingID }));
      await assert.rejects(call("bob", { action: "open", bindingID }));
      await assert.rejects(
        call("operator", { ...grant, subject: "bob" }),
        /runtime/i,
      );
      const launch = (await call("alice", { action: "open", bindingID })) as {
        code: string;
      };
      const session = await exchangeLaunch(db, launch.code);
      await assert.rejects(exchangeLaunch(db, launch.code));
      const access = await inspectSession(db, session.token);
      assert.equal(access.subject, "alice");
      assert.equal(access.profile, "read");
      await db.query(
        "DELETE FROM webdock_auth.member WHERE \"userId\"='alice'",
      );
      await assert.rejects(inspectSession(db, session.token));
      await db.query(
        "INSERT INTO webdock_auth.member VALUES('org-a','alice','admin')",
      );
      const concurrent = (await call("alice", {
        action: "open",
        bindingID,
      })) as { code: string };
      const exchange = async () => {
        const client = await pool.connect();
        try {
          await client.query("BEGIN");
          const result = await exchangeLaunch(client, concurrent.code);
          await client.query("COMMIT");
          return result;
        } catch (error) {
          await client.query("ROLLBACK");
          throw error;
        } finally {
          client.release();
        }
      };
      const raced = await Promise.allSettled([exchange(), exchange()]);
      assert.equal(
        raced.filter((result) => result.status === "fulfilled").length,
        1,
      );
      await db.query(
        `INSERT INTO webdock_auth.studio_tenant_preview VALUES('alice','alice','100','org-a',now()+interval '10 minutes',null)`,
      );
      await assert.rejects(inspectSession(db, session.token));
      await db.query("DELETE FROM webdock_auth.studio_tenant_preview");
      await call("operator", { action: "revoke", bindingID, subject: "alice" });
      await assert.rejects(inspectSession(db, session.token));
      await call("operator", grant);
      const second = (await call("alice", { action: "open", bindingID })) as {
        code: string;
      };
      await db.query(
        "UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id='alice'",
      );
      await assert.rejects(exchangeLaunch(db, second.code));
    } finally {
      db.release();
      await pool.end();
    }
  },
);
