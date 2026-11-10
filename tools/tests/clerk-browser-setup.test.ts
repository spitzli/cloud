import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, writeFile, readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { prepareClerkBrowser } from "./clerk-browser-setup";
test("native Clerk fixture maps explicit owned subject and removes legacy auth without remote requests", async () => {
  const dir = await mkdtemp(join(tmpdir(), "webdock-clerk-setup-"));
  const credentialsPath = join(dir, "credentials.json"),
    developmentEnvPath = join(dir, "dev.env");
  const issuer = "https://on-sheep-3817.clerk.accounts.dev",
    userID = "user_3KqA27SPwkrKlDPfau3Rp8LwR0q";
  const pk =
    "pk_test_" +
    Buffer.from("on-sheep-3817.clerk.accounts.dev$").toString("base64");
  await writeFile(
    credentialsPath,
    JSON.stringify({
      userID,
      issuer,
      appID: "app_3KjGKUGb7dYUMgkjKC0eCWghnOB",
      instanceID: "ins_3KjGKQnAATh2cA4ddJBR2cg3NXu",
      externalID: "webdock-core-acceptance-unit",
      email: "synthetic+clerk_test@example.com",
      password: "unit-fixture-password",
    }),
    { mode: 0o600 },
  );
  await writeFile(
    developmentEnvPath,
    "CLERK_SECRET_KEY=sk_test_unit\nNEXT_PUBLIC_CLERK_PUBLISHABLE_KEY=" + pk,
    { mode: 0o600 },
  );
  const env: Record<string, string | undefined> = {
    NODE_ENV: "development",
    SPITZLI_CONSOLE_ORIGIN: "http://localhost:3460",
    DATABASE_URL: "postgresql://localhost@127.0.0.1:1234/webdock_operator_test",
    BETTER_AUTH_SECRET: "remove",
    SPITZLI_API_DOMAINS: "auth,portal,registry,mcp",
  };
  const calls: unknown[][] = [];
  const owner = {
    query: async (...args: unknown[]) => {
      calls.push(args);
      return {
        rows:
          calls.length === 1
            ? [{ name: "webdock_operator_test" }]
            : [{ user_id: "operator-fixture" }],
      };
    },
  } as never;
  try {
    const result = await prepareClerkBrowser({
      dir,
      owner,
      secrets: { operatorID: "operator-fixture" },
      env,
      credentialsPath,
      developmentEnvPath,
    });
    assert.equal(result.email, "synthetic+clerk_test@example.com");
    assert.equal(env.BETTER_AUTH_SECRET, undefined);
    assert.equal(env.SPITZLI_OAUTH_PROVIDER, "disabled");
    assert.equal(env.SPITZLI_API_DOMAINS, "auth,portal,registry");
    assert.ok(env.SPITZLI_CONSOLE_SERVICE_KEY!.length >= 64);
    assert.deepEqual(JSON.parse(env.SPITZLI_AUTH_PROVIDER!), {
      provider: "clerk",
      issuer,
      authorizedParties: ["http://localhost:3460"],
      allowLocalHTTP: true,
      tokenMode: "session",
    });
    assert.deepEqual(calls[1][1], [issuer, userID, "operator-fixture"]);
    assert.match(calls[1][0] as string, /legacy_eligible/);
    assert.equal(
      JSON.parse(await readFile(join(dir, "clerk-manifest.json"), "utf8"))
        .remoteMutation,
      false,
    );
    await writeFile(
      developmentEnvPath,
      "CLERK_SECRET_KEY=sk_live_forbidden\nNEXT_PUBLIC_CLERK_PUBLISHABLE_KEY=" +
        pk,
    );
    await assert.rejects(
      prepareClerkBrowser({
        dir,
        owner,
        secrets: { operatorID: "operator-fixture" },
        env,
        credentialsPath,
        developmentEnvPath,
      }),
      /development keys/,
    );
    assert.equal(calls.length, 2);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
