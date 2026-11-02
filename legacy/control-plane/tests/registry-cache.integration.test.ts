import { readFile } from "node:fs/promises";
import { randomBytes } from "node:crypto";
import { test, expect, spyOn } from "bun:test";
import pg from "pg";
import { createRegistry } from "../src/modules/registry/service";
import { ViewCache, configureViewCache } from "../src/cache";

const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "registry cache skips data queries but reauthorizes and observes committed writes and related records",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Isolated registry fixture required");
    const db = new pg.Pool({ connectionString: url.href });
    await db.query(
      await readFile(
        new URL("../drizzle/0036_authorized_view_cache.sql", import.meta.url),
        "utf8",
      ),
    );
    const base = BigInt(
      "93" + BigInt("0x" + randomBytes(4).toString("hex")).toString(),
    );
    const [subject, sessionID, native] = [0, 1, 2].map((n) =>
      String(base + BigInt(n)),
    );
    const actor = { subject, sessionID };
    const registry = await createRegistry();
    const store = new Map<string, string>();
    let cacheReads = 0;
    let onGet: (() => Promise<void>) | undefined;
    configureViewCache(
      new ViewCache({
        get: async (key) => {
          cacheReads++;
          await onGet?.();
          return store.get(key) ?? null;
        },
        set: async (key, value) => {
          store.set(key, value);
        },
      }),
      { environment: "registry-test", defaultTTLSeconds: 60 },
    );
    const customers: string[] = [];
    const projects: string[] = [];
    const statements: string[] = [];
    const original = pg.Client.prototype.query;
    const tracked = spyOn(pg.Client.prototype, "query").mockImplementation(
      function (this: pg.Client, ...args: any[]) {
        statements.push(typeof args[0] === "string" ? args[0] : args[0].text);
        return Reflect.apply(original, this, args);
      },
    );
    try {
      await db.query(
        'INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,\'Cache operator\',$2,\'operator\',true,false,true)',
        [subject, subject + "@example.invalid"],
      );
      await db.query(
        'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval \'1 hour\',now())',
        [sessionID, subject],
      );
      await db.query(
        "INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,'Mapped operator',$2,'operator',$3)",
        [native, native + "@example.invalid", subject],
      );
      const first = await registry.writeCustomer(actor, null, {
        name: "Cache customer " + base,
      });
      customers.push(first.id);
      const options = {
        collection: "customers",
        where: { id: { in: customers } },
        depth: 0,
      };
      const initial = await registry.query(actor, "find", options);
      expect(initial.docs).toHaveLength(1);
      statements.length = 0;
      const cached = await registry.query(actor, "find", options);
      expect(cached).toEqual(initial);
      expect(typeof cached.docs[0].createdAt).toBe("string");
      expect(cached.docs[0].updatedAt).toBe(initial.docs[0].updatedAt);
      expect(
        statements.some((sql) =>
          sql.toLowerCase().includes('from "webdock_admin"."customers"'),
        ),
      ).toBe(false);
      expect(
        statements.filter((sql) => sql.includes("webdock_rls.operator()")),
      ).toHaveLength(2);
      expect(
        statements.some((sql) =>
          sql.toLowerCase().includes('from "webdock_admin"."users"'),
        ),
      ).toBe(true);
      expect(store.size).toBeGreaterThan(0);

      await registry.writeCustomer(actor, first.id, {
        name: "Updated cache customer",
      });
      expect((await registry.query(actor, "find", options)).docs[0].name).toBe(
        "Updated cache customer",
      );
      const project = await registry.writeProject(actor, null, {
        name: "Cached project",
        customer: first.id,
      });
      projects.push(project.id);
      const detail = { collection: "projects", id: project.id, depth: 1 };
      expect(
        (await registry.query(actor, "findByID", detail)).customer.name,
      ).toBe("Updated cache customer");
      await db.query(
        "UPDATE webdock_admin.customers SET name='Direct parent update' WHERE id=$1",
        [first.id],
      );
      expect(
        (await registry.query(actor, "findByID", detail)).customer.name,
      ).toBe("Direct parent update");
      const counts = {
        collection: "projects",
        where: { customer: { equals: first.id }, status: { equals: "active" } },
      };
      expect((await registry.query(actor, "count", counts)).totalDocs).toBe(1);
      statements.length = 0;
      expect((await registry.query(actor, "count", counts)).totalDocs).toBe(1);
      expect(
        statements.some((sql) =>
          sql.toLowerCase().includes('from "webdock_admin"."projects"'),
        ),
      ).toBe(false);
      const second = await registry.writeProject(actor, null, {
        name: "Second cached project",
        customer: first.id,
      });
      projects.push(second.id);
      expect((await registry.query(actor, "count", counts)).totalDocs).toBe(2);
      await registry.archive(actor, "projects", second.id, true);
      expect((await registry.query(actor, "count", counts)).totalDocs).toBe(1);
      await db.query("DELETE FROM webdock_admin.projects WHERE id=$1", [
        project.id,
      ]);
      expect((await registry.query(actor, "count", counts)).totalDocs).toBe(0);

      await registry.archive(actor, "projects", second.id, false);
    const instance = await registry.writeInstance(
        actor,
        null,
        {
          label: "Cached CMS",
          project: second.id,
          adminURL: "https://cms.example.invalid",
          schemaName: "cache_" + base,
          provider: "other",
          providerProjectID: "cache-" + base,
          template: "custom",
          status: "retired",
        },
        true,
      );
      const cmsQuery = {
        collection: "cms-instances",
        id: instance.id,
        depth: 2,
      };
      expect(
        (await registry.query(actor, "findByID", cmsQuery)).project.customer
          .name,
      ).toBe("Direct parent update");
      await db.query(
        "UPDATE webdock_admin.customers SET name='Second parent update' WHERE id=$1",
        [first.id],
      );
      expect(
        (await registry.query(actor, "findByID", cmsQuery)).project.customer
          .name,
      ).toBe("Second parent update");
      const audits = {
        collection: "audit-events",
        where: { actor: { equals: native } },
        depth: 1,
      };
      expect(
        (await registry.query(actor, "find", audits)).docs[0].actor.name,
      ).toBe("Mapped operator");
      await db.query(
        "UPDATE webdock_admin.users SET name='Renamed operator' WHERE id=$1",
        [native],
      );
      expect(
        (await registry.query(actor, "find", audits)).docs[0].actor.name,
      ).toBe("Renamed operator");
      const beforeMissing = store.size;
      await expect(
        registry.query(actor, "findByID", {
          collection: "customers",
          id: String(base + 9n),
        }),
      ).rejects.toThrow();
      await expect(
        registry.query(actor, "findByID", {
          collection: "customers",
          id: String(base + 9n),
        }),
      ).rejects.toThrow();
      expect(store.size).toBe(beforeMissing);

      const user = await registry.query(actor, "findByID", {
        collection: "users",
        id: native,
        depth: 0,
      });
      expect(user).not.toHaveProperty("hash");
      expect(JSON.stringify([...store.values()])).not.toContain('"hash":');
      const liveRecord = await registry.getRecord(actor, "customers", first.id);
      expect(liveRecord.createdAt instanceof Date).toBe(true);
      expect(liveRecord.updatedAt instanceof Date).toBe(true);
      // Revoke during the cache lookup: the existing final authorization check must still run.
      await registry.query(actor, "find", options);
      onGet = async () => {
        await db.query(
          'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
          [subject],
        );
      };
      await expect(registry.query(actor, "find", options)).rejects.toThrow();
      onGet = undefined;
      await db.query(
        'UPDATE webdock_auth."user" SET banned=false WHERE id=$1',
        [subject],
      );
      const beforeRevocation = cacheReads;
      await db.query(
        "UPDATE webdock_admin.users SET role='reader' WHERE id=$1",
        [native],
      );
      await expect(registry.query(actor, "find", options)).rejects.toThrow();
      expect(cacheReads).toBe(beforeRevocation);
      await db.query(
        "UPDATE webdock_admin.users SET role='operator' WHERE id=$1",
        [native],
      );
      await db.query(
        'UPDATE webdock_auth."user" SET "twoFactorEnabled"=false WHERE id=$1',
        [subject],
      );
      await expect(registry.query(actor, "find", options)).rejects.toThrow();
      expect(cacheReads).toBe(beforeRevocation);
      await db.query(
        'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
        [subject],
      );
      await db.query("DELETE FROM webdock_auth.session WHERE id=$1", [
        sessionID,
      ]);
      await expect(registry.query(actor, "find", options)).rejects.toThrow();
      expect(cacheReads).toBe(beforeRevocation);
    } finally {
      tracked.mockRestore();
      configureViewCache(undefined, { environment: "test" });
      await registry.close();
      await db.query(
        "DELETE FROM webdock_admin.audit_events WHERE actor_id=$1",
        [native],
      );
      await db.query("DELETE FROM webdock_admin.cms_instances WHERE project_id=ANY($1)", [projects]);
      await db.query("DELETE FROM webdock_admin.projects WHERE id=ANY($1)", [
        projects,
      ]);
      const orgs = (
        await db.query(
          "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1) RETURNING organization_id",
          [customers],
        )
      ).rows.map((row) => row.organization_id);
      await db.query("DELETE FROM webdock_admin.customers WHERE id=ANY($1)", [
        customers,
      ]);
      await db.query("DELETE FROM webdock_auth.organization WHERE id=ANY($1)", [
        orgs,
      ]);
      await db.query("DELETE FROM webdock_admin.users WHERE id=$1", [native]);
      await db.query('DELETE FROM webdock_auth."user" WHERE id=$1', [subject]);
      await db.end();
    }
  },
);
