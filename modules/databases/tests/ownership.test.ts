import { test } from "node:test";
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { createHostingDatabaseBinding } from "../src/server/database-binding";
import { createHostingDatabaseProtection } from "../src/server/database-protection";
import type { DatabaseReader } from "@spitzli/hosting-contracts/runtime-ports";
import { allocationTotals } from "@spitzli/hosting-contracts/capacity-totals";
test("managed database implementation owns its controllers without foreign Hosting server imports", async () => {
  for (const file of await readdir(
    new URL("../src/server/", import.meta.url),
  )) {
    const source = await readFile(
      new URL("../src/server/" + file, import.meta.url),
      "utf8",
    );
    assert.doesNotMatch(
      source,
      /(?:from\s+|import\s*\()["'][^"']*(?:module-hosting|apps\/|\.\/authorization|\.\/clusters|\.\/allowances|\.\/operations|\.\/db["'])/,
    );
  }
  const contract = await readFile(
    new URL(
      "../../../packages/database-contracts/src/runtime-ports.ts",
      import.meta.url,
    ),
    "utf8",
  );
  assert.doesNotMatch(contract, /module-|ReturnType|\bany\b/);
});
test("disabled Databases refuse service binding before credential reads", async () => {
  let queries = 0,
    decrypts = 0;
  const runtime = createHostingDatabaseBinding({
    requireDatabases: () => {
      throw Error("disabled");
    },
    decryptSecret: () => {
      decrypts++;
      throw Error("unexpected");
    },
  });
  const db = {
    query: async () => {
      queries++;
      throw Error("unexpected");
    },
  } as unknown as DatabaseReader;
  await assert.rejects(
    runtime.databaseBindingMaterial(db, {
      databaseID: "1",
      projectID: "2",
      customerID: "3",
      clusterID: "4",
      bindingID: "5",
      variable: "DATABASE_URL",
    }),
    /disabled/,
  );
  assert.equal(queries, 0);
  assert.equal(decrypts, 0);
});
test("database maintenance and shared allocation totals retain conservative capacity rules", () => {
  const runtime = createHostingDatabaseProtection({
    env: { SPITZLI_CONTROL_DATABASE_ID: "123" },
  });
  assert.throws(
    () => runtime.assertDatabaseMaintenanceAllowed("123", "databases.delete"),
    /offline maintenance/,
  );
  assert.doesNotThrow(() =>
    runtime.assertDatabaseMaintenanceAllowed("456", "databases.delete"),
  );
  assert.doesNotThrow(() =>
    runtime.assertDatabaseMaintenanceAllowed("123", "databases.get"),
  );
  const rows = [
    {
      status: "active",
      demand: {
        cpuMillicores: 100,
        memoryBytes: 100,
        volumeBytes: 100,
        ephemeralBytes: 10,
        apps: 1,
        replicasPerApp: 3,
        concurrentDeployments: 1,
      },
    },
    {
      status: "reserved",
      demand: {
        cpuMillicores: 200,
        memoryBytes: 200,
        volumeBytes: 200,
        ephemeralBytes: 20,
        apps: 1,
        replicasPerApp: 2,
        concurrentDeployments: 1,
      },
    },
  ];
  const result = allocationTotals(rows);
  assert.equal(result.cpuMillicores, 300);
  assert.equal(result.replicasPerApp, 3);
  assert.equal(result.concurrentDeployments, 1);
});
