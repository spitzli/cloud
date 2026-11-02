import { configureFullInstallation } from "../../src/full-installation";
import test, { afterEach } from "node:test";
import { mockMethod, restoreMocks } from "../helpers/node-test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import type { ListBlobResult, ListCommandOptions } from "@vercel/blob";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import { createIdentity } from "../../src/modules/auth/bootstrap";
import { tenantSchemaSQL } from "../../src/modules/access/tenant-schema";
import { accessSchemaSQL } from "../../src/modules/access/access-management";
import {
  decryptMailSecret,
  encryptMailSecret,
} from "../../src/modules/platform/platform";
import {
  getTenantStorage,
  manageTenantStorage,
  refreshStorageBatch,
  measureStorage,
  normalizeStoragePrefix,
  storageUsageSchemaSQL,
} from "../../src/modules/platform/storage-usage";
afterEach(restoreMocks);
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
configureFullInstallation();
const token = "vercel_blob_rw_teststore_testsecret";
function blob(pathname: string, size: number) {
  return {
    pathname,
    size,
    url: `https://teststore.public.blob.vercel-storage.com/${pathname}`,
    downloadUrl: "",
    uploadedAt: new Date(),
    etag: "",
  };
}
const page = (pathname = "project/image.webp", size = 12): ListBlobResult => ({
  blobs: [blob(pathname, size)],
  hasMore: false,
});
async function identity(operator = false) {
  const password = randomBytes(24).toString("base64url");
  const user = await createIdentity({
    email: `storage-${randomBytes(8).toString("hex")}@example.invalid`,
    name: "Storage test",
    password,
    operator,
    mustChangePassword: false,
  });
  const origin = process.env.BETTER_AUTH_URL!;
  const response = await auth.handler(
    new Request(origin + "/api/auth/sign-in/email", {
      method: "POST",
      headers: {
        Origin: origin,
        "Content-Type": "application/json",
        "x-webdock-client-ip": `192.0.2.${1 + (randomBytes(1)[0] % 250)}`,
      },
      body: JSON.stringify({ email: user.email, password }),
    }),
  );
  assert.equal(response.status, 200);
  if (operator)
    await database.query(
      'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
      [user.id],
    );
  return {
    ...user,
    headers: new Headers({
      Origin: origin,
      Cookie: response.headers
        .getSetCookie()
        .map((c) => c.split(";")[0])
        .join("; "),
    }),
  };
}
test("Inventory includes paginated variants and rejects incomplete, unsafe and cross-prefix results", async () => {
  assert.equal(normalizeStoragePrefix(" project/// "), "project/");
  assert.equal(normalizeStoragePrefix(""), "");
  for (const input of ["/", "/project", "../project", "x//y", "a\\b", "x?y"])
    assert.throws(() => normalizeStoragePrefix(input));
  const seen: Array<string | undefined> = [];
  assert.deepEqual(
    await measureStorage("store_teststore", token, "project/", {
      list: async (options) => {
        seen.push(options.cursor);
        assert.equal(options.prefix, "project/");
        return options.cursor
          ? page("project/variant.webp", 5)
          : { ...page(), cursor: "next", hasMore: true };
      },
    }),
    { bytes: 17, objects: 2 },
  );
  assert.deepEqual(seen, [undefined, "next"]);
  assert.deepEqual(
    await measureStorage("store_teststore", token, "project/", {
      list: async () => ({ blobs: [], hasMore: false }),
    }),
    { bytes: 0, objects: 0 },
  );
  for (const result of [
    page("project-other/a"),
    page("project/a", -1),
    page("project/a", Number.MAX_SAFE_INTEGER + 1),
    { ...page(), hasMore: true },
    {
      ...page(),
      blobs: [
        {
          ...blob("project/a", 1),
          url: "https://wrong.public.blob.vercel-storage.com/a",
        },
      ],
    },
  ])
    await assert.rejects(
      measureStorage("store_teststore", token, "project/", {
        list: async () => result,
      }),
    );
  await assert.rejects(
    measureStorage("store_wrong", token, "", { list: async () => page() }),
    /matching/,
  );
  await assert.rejects(
    measureStorage("store_teststore", token, "project/", {
      list: async () => ({ ...page(), hasMore: true, cursor: "repeat" }),
    }),
  );
  let count = 0;
  await assert.rejects(
    measureStorage("store_teststore", token, "project/", {
      list: async () => ({ blobs: [], hasMore: true, cursor: String(++count) }),
    }),
  );
  assert.equal(count, 100);
});
test("Storage mappings enforce tenant/operator access, overlap safety, stale snapshots and concurrent refresh guards", async () => {
  await database.query(tenantSchemaSQL);
  await database.query(accessSchemaSQL);
  await database.query(storageUsageSchemaSQL);
  const root = await identity(true),
    admin = await identity(),
    stranger = await identity();
  const tenants = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Storage A'),(webdock_auth.next_snowflake(),'Storage B') RETURNING id",
    )
  ).rows;
  const [one, two] = tenants.map((row) => row.id);
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [one],
    )
  ).rows[0].organization_id;
  await database.query(
    'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
    [org, admin.id],
  );
  const store = randomBytes(8).toString("hex"),
    storeID = `store_${store}`,
    secret = `vercel_blob_rw_${store}_testsecret`;
  const add = {
    action: "add",
    label: "Explicit project",
    environment: "production",
    prefix: "project",
    storeID,
    token: secret,
  };
  const listing = (prefix: string, size = 12) => ({
    list: async (
      options: ListCommandOptions<"expanded">,
    ): Promise<ListBlobResult> => {
      assert.equal(options.prefix, prefix);
      return {
        blobs: [
          {
            ...blob(`${prefix}image.webp`, size),
            url: `https://${store}.public.blob.vercel-storage.com/${prefix}image.webp`,
          },
        ],
        hasMore: false,
      };
    },
  });
  try {
    assert.equal(
      (await getTenantStorage(admin.headers, one)).productionBytes,
      null,
    );
    await assert.rejects(getTenantStorage(admin.headers, two));
    await assert.rejects(getTenantStorage(stranger.headers, one));
    for (const action of ["add", "refresh", "remove"])
      await assert.rejects(
        manageTenantStorage(
          admin.headers,
          one,
          { ...add, action },
          listing("project/"),
        ),
        /operator/,
      );
    await manageTenantStorage(root.headers, one, add, listing("project/"));
    let view = await getTenantStorage(admin.headers, one);
    const id = view.stores[0].id;
    assert.equal(view.productionBytes, 12);
    assert.equal(view.previewBytes, null);
    assert.equal(view.stores[0].prefix, "project/");
    assert.equal(JSON.stringify(view).includes(secret), false);
    const stored = (
      await database.query(
        "SELECT encrypted_token FROM webdock_auth.tenant_storage WHERE id=$1",
        [id],
      )
    ).rows[0];
    assert.notEqual(stored.encrypted_token, secret);
    assert.equal(
      decryptMailSecret(stored.encrypted_token, `storage:${id}`),
      secret,
    );
    for (const prefix of ["", "project/", "project/sub/"])
      await assert.rejects(
        manageTenantStorage(
          root.headers,
          two,
          { ...add, prefix },
          listing(prefix),
        ),
        /already mapped/,
      );
    await manageTenantStorage(
      root.headers,
      two,
      { ...add, prefix: "project-other/" },
      listing("project-other/"),
    );
    await manageTenantStorage(
      root.headers,
      one,
      { ...add, environment: "preview", prefix: "preview/" },
      listing("preview/", 7),
    );
    assert.equal((await getTenantStorage(admin.headers, one)).previewBytes, 7);
    const refresh = { action: "refresh", storageID: id };
    await assert.rejects(
      manageTenantStorage(root.headers, two, refresh),
      /not found/,
    );
    const checkedAt = view.stores[0].checkedAt;
    await assert.rejects(
      manageTenantStorage(root.headers, one, refresh, {
        list: async () => {
          throw Error(secret);
        },
      }),
      /incomplete/,
    );
    view = await getTenantStorage(admin.headers, one);
    const stale = view.stores.find((s) => s.id === id)!;
    assert.equal(view.productionBytes, null);
    assert.equal(stale.bytes, 12);
    assert.equal(stale.checkedAt, checkedAt);
    assert.ok(stale.error);
    assert.equal(JSON.stringify(view).includes(secret), false);
    await manageTenantStorage(
      root.headers,
      one,
      refresh,
      listing("project/", 25),
    );
    assert.equal(
      (await getTenantStorage(admin.headers, one)).productionBytes,
      25,
    );
    await assert.rejects(
      manageTenantStorage(root.headers, one, refresh, {
        list: async (options) => {
          await database.query(
            'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
            [root.id],
          );
          return listing("project/", 99).list(options);
        },
      }),
    );
    await database.query(
      'UPDATE webdock_auth."user" SET banned=false WHERE id=$1',
      [root.id],
    );
    assert.equal(
      (await getTenantStorage(root.headers, one)).productionBytes,
      25,
    );
    await assert.rejects(
      manageTenantStorage(root.headers, one, refresh, {
        list: async (options) => {
          await manageTenantStorage(
            root.headers,
            one,
            refresh,
            listing("project/", 40),
          );
          return listing("project/", 99).list(options);
        },
      }),
      /changed/,
    );
    assert.equal(
      (await getTenantStorage(root.headers, one)).productionBytes,
      40,
    );
    await assert.rejects(
      manageTenantStorage(root.headers, one, refresh, {
        list: async (options) => {
          await manageTenantStorage(root.headers, one, {
            action: "remove",
            storageID: id,
          });
          return listing("project/", 99).list(options);
        },
      }),
      /changed/,
    );
    assert.equal(
      (await getTenantStorage(root.headers, one)).productionBytes,
      null,
    );
    await database.query(
      "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
      [one],
    );
    await assert.rejects(getTenantStorage(admin.headers, one));
    await assert.rejects(
      manageTenantStorage(root.headers, one, add, listing("project/")),
      /Active tenant/,
    );
  } finally {
    await database.query(
      "DELETE FROM webdock_auth.tenant_storage WHERE customer_id=ANY($1)",
      [[one, two]],
    );
    await database.query(
      'UPDATE webdock_auth."user" SET banned=false WHERE id=$1',
      [root.id],
    );
  }
});

test("Scheduled batches reserve disjoint bounded work, skip archived tenants, preserve failures and never resurrect removals", async () => {
  const connection = await database.connect();
  const query = mockMethod(
    database,
    "query",
    connection.query.bind(connection),
  );
  try {
    await connection.query("BEGIN");
    await connection.query(storageUsageSchemaSQL);
    // Fixture isolation is transaction-local: browser fixtures are unchanged after rollback.
    await connection.query(
      "UPDATE webdock_auth.tenant_storage SET last_attempt_at=now()",
    );
    const customer = (
      await connection.query(
        "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Batch fixture') RETURNING id",
      )
    ).rows[0].id;
    const archived = (
      await connection.query(
        "INSERT INTO webdock_admin.customers(id,name,status) VALUES(webdock_auth.next_snowflake(),'Archived batch fixture','archived') RETURNING id",
      )
    ).rows[0].id;
    const ids: string[] = [];
    for (let i = 0; i < 13; i++) {
      const id = `batch-${randomBytes(8).toString("hex")}`;
      ids.push(id);
      await connection.query(
        "INSERT INTO webdock_auth.tenant_storage(id,customer_id,label,environment,store_id,prefix,encrypted_token,bytes,objects,checked_at,last_attempt_at) VALUES($1,$2,$3,'production','store_teststore',$4,$5,50,1,now()-interval '2 days',NULL)",
        [
          id,
          i === 12 ? archived : customer,
          `Batch ${i}`,
          `batch/${i}/`,
          encryptMailSecret(token, `storage:${id}`),
        ],
      );
    }
    const seen = new Set<string>();
    let secondBatch: ReturnType<typeof refreshStorageBatch> | undefined;
    let active = 0,
      maximum = 0;
    const dependencies = {
      list: async (
        options: ListCommandOptions<"expanded">,
      ): Promise<ListBlobResult> => {
        active++;
        maximum = Math.max(maximum, active);
        try {
          const prefix = options.prefix!;
          if (!options.cursor) {
            assert.equal(seen.has(prefix), false);
            seen.add(prefix);
          }
          if (!secondBatch) secondBatch = refreshStorageBatch(dependencies);
          if (prefix === "batch/0/") throw Error("SECRET must not leak");
          if (prefix === "batch/1/")
            await connection.query(
              "DELETE FROM webdock_auth.tenant_storage WHERE id=$1",
              [ids[1]],
            );
          await new Promise((resolve) => setImmediate(resolve));
          return options.cursor
            ? page(`${prefix}variant.webp`, 3)
            : {
                ...page(`${prefix}original.webp`, 5),
                cursor: "next",
                hasMore: true,
              };
        } finally {
          active--;
        }
      },
    };
    const first = await refreshStorageBatch(dependencies),
      second = await secondBatch!;
    assert.equal(first.attempted, 10);
    assert.equal(second.attempted, 2);
    assert.equal(first.succeeded + second.succeeded, 10);
    assert.equal(first.failed + second.failed, 2);
    assert.equal(seen.size, 12);
    assert.equal(seen.has("batch/12/"), false);
    assert.ok(maximum <= 4);
    const rows = (
      await connection.query(
        "SELECT * FROM webdock_auth.tenant_storage WHERE customer_id=$1",
        [customer],
      )
    ).rows;
    const failed = rows.find((row) => row.id === ids[0]);
    assert.equal(failed.bytes, "50");
    assert.equal(failed.objects, 1);
    assert.ok(failed.error);
    assert.ok(!failed.error.includes("SECRET"));
    assert.ok(new Date(failed.checked_at).getTime() < Date.now() - 86_400_000);
    assert.ok(failed.last_attempt_at);
    assert.equal(
      rows.some((row) => row.id === ids[1]),
      false,
    );
    assert.ok(
      rows
        .filter((row) => row.id !== ids[0])
        .every((row) => row.bytes === "8" && row.objects === 2 && !row.error),
    );
    assert.deepEqual(await refreshStorageBatch(dependencies), {
      attempted: 0,
      succeeded: 0,
      failed: 0,
    });
    await connection.query(
      "UPDATE webdock_auth.tenant_storage SET last_attempt_at=now()-interval '25 hours' WHERE customer_id=$1",
      [customer],
    );
    seen.clear();
    maximum = 0;
    assert.equal((await refreshStorageBatch(dependencies)).attempted, 10);
    assert.ok(maximum <= 2);
  } finally {
    await connection.query("ROLLBACK");
    query.mock.restore();
    connection.release();
  }
});
