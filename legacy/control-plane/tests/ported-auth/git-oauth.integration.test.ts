import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { database } from "../../src/db/runtime";
import { gitDeploymentSchemaSQL } from "../../src/modules/hosting/git-deployment-schema";
import { executeGitOAuth } from "../../src/modules/hosting/git-oauth";
import { createGitHubProvider } from "../../src/modules/hosting/git-github";
import type { HostingActor } from "@webdock/hosting-contracts";
const url = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(url.hostname) ||
  url.pathname != "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local DB required");
const key = () => randomBytes(16).toString("hex");
test.after(() => database.end());
test("Git OAuth flows bind tenant, actor and session and consume code/finish exactly once", async (t) => {
  await database.query(gitDeploymentSchemaSQL);
  const variables = {
    WEBDOCK_GITHUB_CLIENT_ID: "Iv1.fixture",
    WEBDOCK_GITHUB_APP_SLUG: "webdock-fixture",
    WEBDOCK_GIT_STUDIO_ORIGIN: "https://studio.example",
  };
  const previous = Object.fromEntries(
    Object.keys(variables).map((k) => [k, process.env[k]]),
  );
  Object.assign(process.env, variables);
  t.after(() => {
    for (const [k, v] of Object.entries(previous))
      if (v === undefined) delete process.env[k];
      else process.env[k] = v;
  });
  const user = (
    await database.query(
      `INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword","createdAt","updatedAt") VALUES(webdock_auth.next_snowflake(),'OAuth fixture',$1,true,'operator',true,false,now(),now()) RETURNING id`,
      [`oauth-${key()}@example.invalid`],
    )
  ).rows[0].id;
  const sessions = [] as string[];
  for (let i = 0; i < 2; i++)
    sessions.push(
      (
        await database.query(
          `INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","createdAt","updatedAt") VALUES(webdock_auth.next_snowflake(),$1,$2,now()+interval '1 hour',now(),now()) RETURNING id`,
          [key(), user],
        )
      ).rows[0].id,
    );
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'OAuth fixture') RETURNING id",
    )
  ).rows[0].id;
  const actor: HostingActor = {
    subject: user,
    sessionID: sessions[0],
    source: "studio",
    scopes: ["hosting:read", "hosting:write"],
  };
  const actorOtherSession = { ...actor, sessionID: sessions[1] };
  const binding = {
    installationID: String(Date.now()),
    repositoryID: "987654321",
    accountID: "111",
    accountLogin: "oauth-fixture",
    owner: "fixture",
    name: "repo",
    fullName: "fixture/repo",
    defaultBranch: "main",
    permissions: { contents: "read", checks: "write" },
  };
  let exchanges = 0,
    verifications = 0;
  const provider = {
    exchangeOAuthCode: async () => {
      exchanges++;
      return "private-fixture-token";
    },
    verifyRepository: async (
      token: string,
      installationID: string,
      repositoryID: string,
    ) => {
      verifications++;
      assert.equal(token, "private-fixture-token");
      assert.equal(installationID, binding.installationID);
      assert.equal(repositoryID, binding.repositoryID);
      return binding;
    },
    listUserInstallations: async () => [
      {
        installationID: binding.installationID,
        accountID: binding.accountID,
        accountLogin: binding.accountLogin,
      },
    ],
    listUserRepositories: async () => [binding],
  } as unknown as ReturnType<typeof createGitHubProvider>;
  const project = (
    await database.query(
      "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES(webdock_auth.next_snowflake(),'OAuth setup',$1,'active') RETURNING id",
      [customer],
    )
  ).rows[0].id;
  const setup = (
    await database.query(
      "INSERT INTO webdock_auth.hosting_setup(customer_id,project_id,subject,configuration) VALUES($1,$2,$3,$4) RETURNING id",
      [
        customer,
        project,
        user,
        JSON.stringify({
          name: "web",
          source: { kind: "git", branch: "main", rootDirectory: "." },
          spec: {
            args: [],
            port: 3000,
            healthPath: "/",
            cpuMillicores: 100,
            memoryBytes: 67108864,
            ephemeralBytes: 67108864,
            replicas: 1,
          },
          database: { mode: "none" },
          mail: null,
          autoPublish: false,
        }),
      ],
    )
  ).rows[0].id;
  await assert.rejects(
    executeGitOAuth(
      actor,
      { action: "git.oauth.begin", customerID: customer, setupID: "1" },
      provider,
    ),
  );
  const begin = await executeGitOAuth(
    actor,
    { action: "git.oauth.begin", customerID: customer, setupID: setup },
    provider,
  );
  const state = "state" in begin ? String(begin.state) : "";
  assert.match(state, /^[A-Za-z0-9_-]{43}$/);
  const context = await executeGitOAuth(
    actor,
    { action: "git.oauth.return", state },
    provider,
  );
  assert.equal(
    context.returnPath,
    `/hosting/projects/${project}/setup/${setup}`,
  );
  await assert.rejects(
    executeGitOAuth(
      actorOtherSession,
      { action: "git.oauth.return", state },
      provider,
    ),
  );
  const exchange = {
    action: "git.oauth.exchange" as const,
    state,
    code: "fixture-code",
  };
  await assert.rejects(executeGitOAuth(actorOtherSession, exchange, provider));
  assert.equal(exchanges, 0);
  await assert.rejects(
    executeGitOAuth({ ...actor, subject: "123" }, exchange, provider),
  );
  assert.equal(exchanges, 0);
  await assert.rejects(
    executeGitOAuth({ ...actor, source: "oauth" }, exchange, provider),
  );
  assert.equal(exchanges, 0);
  await assert.rejects(
    executeGitOAuth(actor, { action: "git.oauth.options", state }, provider),
  );
  const competing = await Promise.allSettled([
    executeGitOAuth(actor, exchange, provider),
    executeGitOAuth(actor, exchange, provider),
  ]);
  assert.equal(competing.filter((v) => v.status === "fulfilled").length, 1);
  assert.equal(competing.filter((v) => v.status === "rejected").length, 1);
  assert.equal(exchanges, 1);
  const stored = (
    await database.query(
      "SELECT user_token_encrypted FROM webdock_auth.git_flow WHERE subject=$1 AND consumed_at IS NULL",
      [user],
    )
  ).rows[0];
  assert.ok(stored.user_token_encrypted);
  assert.ok(!stored.user_token_encrypted.includes("private-fixture-token"));
  await assert.rejects(
    executeGitOAuth(
      actorOtherSession,
      { action: "git.oauth.options", state },
      provider,
    ),
  );
  const finish = {
    action: "git.oauth.finish" as const,
    state,
    installationID: binding.installationID,
    repositoryID: binding.repositoryID,
  };
  await assert.rejects(executeGitOAuth(actorOtherSession, finish, provider));
  assert.equal(verifications, 0);
  const finishes = await Promise.allSettled([
    executeGitOAuth(actor, finish, provider),
    executeGitOAuth(actor, finish, provider),
  ]);
  assert.equal(finishes.filter((v) => v.status === "fulfilled").length, 1);
  assert.equal(finishes.filter((v) => v.status === "rejected").length, 1);
  assert.equal(verifications, 1);
  const connected = (
    finishes.find((v) => v.status === "fulfilled") as PromiseFulfilledResult<
      Awaited<ReturnType<typeof executeGitOAuth>>
    >
  ).value;
  assert.equal(connected.customerID, customer);
  assert.ok(connected.connection);
  assert.equal(connected.connection.customerID, customer);
  assert.equal(
    connected.returnPath,
    `/hosting/projects/${project}/setup/${setup}`,
  );
  const restored = (
    await database.query(
      "SELECT configuration,revision FROM webdock_auth.hosting_setup WHERE id=$1",
      [setup],
    )
  ).rows[0];
  assert.equal(
    restored.configuration.source.connectionID,
    connected.connection.id,
  );
  assert.equal(
    restored.configuration.source.repositoryID,
    binding.repositoryID,
  );
  assert.equal(restored.revision, 2);
  const consumed = (
    await database.query(
      "SELECT consumed_at,user_token_encrypted FROM webdock_auth.git_flow WHERE subject=$1",
      [user],
    )
  ).rows[0];
  assert.ok(consumed.consumed_at);
  assert.equal(consumed.user_token_encrypted, null);
  await assert.rejects(executeGitOAuth(actor, exchange, provider));
  await assert.rejects(executeGitOAuth(actor, finish, provider));
  const expiring = await executeGitOAuth(
    actor,
    { action: "git.oauth.begin", customerID: customer },
    provider,
  );
  const expiredState = "state" in expiring ? String(expiring.state) : "";
  await database.query(
    "UPDATE webdock_auth.git_flow SET expires_at=now()-interval '1 second' WHERE subject=$1 AND consumed_at IS NULL",
    [user],
  );
  await assert.rejects(
    executeGitOAuth(
      actor,
      {
        action: "git.oauth.exchange",
        state: expiredState,
        code: "fixture-code",
      },
      provider,
    ),
  );
  assert.equal(exchanges, 1);
  const valid = await executeGitOAuth(
    actor,
    { action: "git.oauth.begin", customerID: customer },
    provider,
  );
  const validState = "state" in valid ? String(valid.state) : "";
  await database.query(
    "UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id=$1",
    [sessions[0]],
  );
  await assert.rejects(
    executeGitOAuth(
      actor,
      { action: "git.oauth.exchange", state: validState, code: "fixture-code" },
      provider,
    ),
  );
  assert.equal(exchanges, 1);
});
