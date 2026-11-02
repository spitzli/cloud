import { test } from "node:test";
import assert from "node:assert/strict";
import {
  currentDatabaseScope,
  withDatabaseScope,
} from "@webdock/database-runtime";
import {
  withHostingDatabase,
  withMailDatabase,
} from "../../src/db/database-context";

test("parallel hosting callers retain their own sessions and restore the outer scope", async () => {
  await Promise.all(
    ["alice", "bob"].map((subject) =>
      withHostingDatabase(
        {
          subject,
          sessionID: subject + "-session",
          source: "studio",
          scopes: ["hosting:read"],
        },
        async () => {
          await new Promise((resolve) => setImmediate(resolve));
          assert.deepEqual(currentDatabaseScope(), {
            kind: "session",
            authProvider: "better-auth",
            subject,
            sessionID: subject + "-session",
          });
        },
      ),
    ),
  );
  assert.equal(currentDatabaseScope(), undefined);
});

test("Mail API keys bind the connection and key as well as the tenant", async () => {
  await withMailDatabase(
    {
      customerID: "101",
      connectionID: "202",
      principal: { kind: "api_key", keyID: "303" },
    },
    async () => {
      assert.deepEqual(currentDatabaseScope(), {
        kind: "mail-key",
        customerID: "101",
        connectionID: "202",
        keyID: "303",
      });
    },
  );
  assert.equal(currentDatabaseScope(), undefined);
});

test("Git policy and job actors cannot manufacture worker authority", async () => {
  const git = {
    subject: "alice",
    sessionID: "",
    source: "git-policy" as const,
    scopes: ["hosting:read" as const],
  };
  await withHostingDatabase(git, async () =>
    assert.equal(currentDatabaseScope(), undefined),
  );
  const job = {
    customerID: "101",
    connectionID: "202",
    principal: { kind: "job" as const, jobID: "303" },
  };
  await withMailDatabase(job, async () =>
    assert.equal(currentDatabaseScope(), undefined),
  );
  await withDatabaseScope({ kind: "system", purpose: "git-worker" }, () =>
    withHostingDatabase(git, async () => {
      assert.deepEqual(currentDatabaseScope(), {
        kind: "system",
        purpose: "git-worker",
      });
      await assert.rejects(
        withMailDatabase(job, async () => {
          throw Error("failed");
        }),
        /failed/,
      );
      assert.deepEqual(currentDatabaseScope(), {
        kind: "system",
        purpose: "git-worker",
      });
    }),
  );
  assert.equal(currentDatabaseScope(), undefined);
});

test("Hosting retains plugin invocation only for the same verified session", async () => {
  const session = {
    kind: "session" as const,
    subject: "alice",
    sessionID: "alice-session",
    pluginInvocationID: "invocation",
  };
  await withDatabaseScope(session, async () => {
    for (const sessionID of ["alice-session", "different-session"]) {
      await withHostingDatabase(
        {
          subject: "alice",
          sessionID,
          source: "studio",
          scopes: ["hosting:read"],
        },
        async () => {
          const scope = currentDatabaseScope();
          assert.equal(
            scope?.kind === "session" ? scope.pluginInvocationID : undefined,
            sessionID === "alice-session" ? "invocation" : undefined,
          );
        },
      );
    }
  });
});
