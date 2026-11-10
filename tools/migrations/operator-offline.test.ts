import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mkdtemp,
  readFile,
  stat,
  rm,
  writeFile,
  symlink,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { persistInitialOperator, OperatorRunError } from "./operator-offline";
import type { Pool } from "pg";

const secrets = {
  operatorID: "123",
  portalUserID: "456",
  bindingID: "789",
  clientID: "321",
  checksum: "metadata",
  environment: { SPITZLI_SSO_CLIENT_SECRET: "private-client-secret" },
};
function database(commitFails = false) {
  const statements: string[] = [];
  return {
    statements,
    pool: {
      connect: async () => ({
        query: async (sql: string) => {
          statements.push(sql);
          if (sql === "COMMIT" && commitFails)
            throw Error("SQL private-diagnostics");
          return { rows: [{ name: "webdock" }] };
        },
        release() {},
      }),
      end: async () => {},
    } as unknown as Pool,
  };
}

test("offline runner durably writes private credentials before commit and exposes only metadata", async () => {
  const dir = await mkdtemp(join(tmpdir(), "operator-runner-"));
  try {
    const output = join(dir, "secrets.json"),
      db = database();
    const result = await persistInitialOperator(
      db.pool,
      "webdock",
      output,
      async () => secrets,
    );
    assert.deepEqual(result, {
      created: true,
      operatorID: "123",
      clientID: "321",
      checksum: "metadata",
      securitySetupRequired: true,
    });
    assert.equal((await stat(output)).mode & 0o777, 0o600);
    assert.equal(
      JSON.parse(await readFile(output, "utf8")).environment
        .SPITZLI_SSO_CLIENT_SECRET,
      "private-client-secret",
    );
    assert.equal(db.statements.at(-1), "COMMIT");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("offline runner refuses existing files and symlinks without starting database work", async () => {
  const dir = await mkdtemp(join(tmpdir(), "operator-runner-"));
  try {
    const existing = join(dir, "existing"),
      link = join(dir, "link");
    await writeFile(existing, "untouched");
    await symlink(existing, link);
    for (const output of [existing, link]) {
      const db = database();
      await assert.rejects(
        persistInitialOperator(db.pool, "webdock", output, async () => secrets),
      );
      assert.equal(db.statements.length, 0);
    }
    assert.equal(await readFile(existing, "utf8"), "untouched");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("offline runner deletes uncommitted output but retains recovery secrets after uncertain commit", async () => {
  const dir = await mkdtemp(join(tmpdir(), "operator-runner-"));
  try {
    const output = join(dir, "secrets.json"),
      db = database();
    await assert.rejects(
      persistInitialOperator(db.pool, "webdock", output, async () => {
        throw Error("SQL private-diagnostics");
      }),
      (error: unknown) => {
        assert.ok(error instanceof OperatorRunError);
        assert.equal(error.commitUncertain, false);
        assert.ok(!error.message.includes("private-diagnostics"));
        return true;
      },
    );
    await assert.rejects(stat(output), { code: "ENOENT" });
    await assert.rejects(
      persistInitialOperator(
        database(true).pool,
        "webdock",
        output,
        async () => secrets,
      ),
      (error: unknown) => {
        assert.ok(error instanceof OperatorRunError);
        assert.equal(error.commitUncertain, true);
        assert.match(error.message, /retain the private output/);
        return true;
      },
    );
    assert.equal(JSON.parse(await readFile(output, "utf8")).operatorID, "123");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
