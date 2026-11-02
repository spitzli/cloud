import { test, expect } from "bun:test";
import { existsSync } from "node:fs";
import { generateKeyPair, exportJWK, createLocalJWKSet, SignJWT } from "jose";
import { pluginInstallerProtocol } from "../src/protocols/plugin-installer";
const file = new URL(
  "../src/modules/plugins/sources-actions.ts",
  import.meta.url,
);
const actions = existsSync(file) ? await import(file.pathname) : null;
const metadata = {
  repositoryID: "123",
  repository: "owner/private-builder",
  workflowRef:
    "owner/private-builder/.github/workflows/plugin.yml@refs/heads/main",
  workflowSHA: "a".repeat(40),
  nonce: "b".repeat(32),
  size: 10,
  sourceSHA256: "c".repeat(64),
  runID: "456",
};
const input = { id: "10", nonce: metadata.nonce, offset: 0 };
const claims = {
  repository_id: "123",
  repository: "owner/private-builder",
  repository_visibility: "private",
  workflow_ref: metadata.workflowRef,
  workflow_sha: metadata.workflowSHA,
  run_id: "456",
  run_attempt: "1",
  event_name: "workflow_dispatch",
};

test("Actions source JWT requires fixed issuer, exact audience, RS256 and live expiry using verified JWKS", async () => {
  expect(actions).not.toBeNull();
  const pair = await generateKeyPair("RS256"),
    other = await generateKeyPair("RS256");
  const key = {
    ...(await exportJWK(pair.publicKey)),
    kid: "fixture",
    alg: "RS256",
    use: "sig",
  };
  const verify = actions.createGitHubActionsVerifier(
    createLocalJWKSet({ keys: [key] }),
  );
  const issue = (
    overrides: Record<string, unknown> = {},
    privateKey = pair.privateKey,
  ) =>
    new SignJWT({
      ...claims,
      iss: "https://token.actions.githubusercontent.com",
      aud: "webdock-plugin-build:10:" + metadata.nonce,
      iat: Math.floor(Date.now() / 1000),
      nbf: Math.floor(Date.now() / 1000) - 1,
      exp: Math.floor(Date.now() / 1000) + 300,
      ...overrides,
    })
      .setProtectedHeader({ alg: "RS256", kid: "fixture" })
      .sign(privateKey);
  expect((await verify("Bearer " + (await issue()), input)).repository_id).toBe(
    "123",
  );
  for (const changed of [
    { iss: "https://evil.example" },
    { aud: "webdock-plugin-build:11:" + metadata.nonce },
    { aud: ["webdock-plugin-build:10:" + metadata.nonce, "extra"] },
    { exp: 1 },
    { repository_visibility: "public" },
    { run_attempt: "2" },
    { event_name: "pull_request" },
  ])
    await expect(
      verify("Bearer " + (await issue(changed)), input),
    ).rejects.toMatchObject({ status: 401 });
  await expect(
    verify("Bearer " + (await issue({}, other.privateKey)), input),
  ).rejects.toMatchObject({ status: 401 });
  await expect(
    verify("Bearer global-installer-token", input),
  ).rejects.toMatchObject({ status: 401 });
});
test("Actions source identity matches every prepared repository/workflow/run field before returning bytes", () => {
  expect(actions).not.toBeNull();
  expect(() =>
    actions.requireActionsIdentity(metadata, claims, input),
  ).not.toThrow();
  for (const changed of [
    { repository_id: "999" },
    { repository: "other/private-builder" },
    { workflow_ref: metadata.workflowRef + "x" },
    { workflow_sha: "d".repeat(40) },
    { run_id: "999" },
    { run_attempt: "2" },
    { repository_visibility: "public" },
    { event_name: "push" },
  ])
    expect(() =>
      actions.requireActionsIdentity(
        metadata,
        { ...claims, ...changed },
        input,
      ),
    ).toThrow();
  expect(() =>
    actions.requireActionsIdentity(metadata, claims, {
      ...input,
      nonce: "e".repeat(32),
    }),
  ).toThrow();
  try {
    actions.requireActionsIdentity(
      { ...metadata, runID: undefined },
      claims,
      input,
    );
    throw Error("should wait");
  } catch (error) {
    expect(error).toMatchObject({
      status: 503,
      code: "actions_binding_pending",
    });
  }
});
test("Actions HTTP source accepts only OIDC while installer mutation routes retain their own token", async () => {
  expect(actions).not.toBeNull();
  const pair = await generateKeyPair("RS256"),
    verify = actions.createGitHubActionsVerifier(
      createLocalJWKSet({
        keys: [
          {
            ...(await exportJWK(pair.publicKey)),
            kid: "http-fixture",
            alg: "RS256",
          },
        ],
      }),
    );
  const jwt = await new SignJWT(claims)
    .setProtectedHeader({ alg: "RS256", kid: "http-fixture" })
    .setIssuer("https://token.actions.githubusercontent.com")
    .setAudience("webdock-plugin-build:10:" + metadata.nonce)
    .setIssuedAt()
    .setNotBefore("0s")
    .setExpirationTime("5m")
    .sign(pair.privateKey);
  const old = {
    token: process.env.WEBDOCK_PLUGIN_INSTALLER_TOKEN,
    file: process.env.WEBDOCK_PLUGIN_RUNTIME_FILE,
  };
  const token = "installer-fixture-" + "x".repeat(40);
  process.env.WEBDOCK_PLUGIN_INSTALLER_TOKEN = token;
  process.env.WEBDOCK_PLUGIN_RUNTIME_FILE = "/runtime-fixture";
  let mutations = 0,
    reads = 0;
  const { app } = pluginInstallerProtocol(
    {
      apiOrigin: "https://api.example",
      authOrigin: "https://auth.example",
      studioOrigin: "https://studio.example",
      production: false,
      domains: ["registry"],
      port: 3130,
    },
    async () => {
      mutations++;
      return { accepted: true };
    },
    async (header, raw) => {
      const request = actions.actionsSourceInput.parse(raw),
        identity = await verify(header, request);
      actions.requireActionsIdentity(metadata, identity, request);
      reads++;
      return {
        data: "eA==",
        done: true,
        sourceSHA256: metadata.sourceSHA256,
        projectID: "2",
        customerID: "3",
      };
    },
  );
  const call = (action: string, bearer: string, body: unknown = input) =>
    app.handle(
      new Request("https://api.example/v1/plugin-installer/" + action, {
        method: "POST",
        headers: {
          authorization: "Bearer " + bearer,
          "content-type": "application/json",
        },
        body: JSON.stringify(body),
      }),
    );
  try {
    expect((await call("actions-source", token)).status).toBe(401);
    expect(reads).toBe(0);
    const result = await call("actions-source", jwt);
    expect(result.status).toBe(200);
    expect(result.headers.get("cache-control")).toBe("no-store");
    expect((await result.json()).data).toBe("eA==");
    expect(reads).toBe(1);
    expect(mutations).toBe(0);
    expect((await call("claim", jwt, {})).status).toBe(401);
    expect(mutations).toBe(0);
    for (const action of ["actions-prepare", "actions-chunk", "actions-bind"])
      expect((await call(action, token, {})).status).toBe(200);
    expect(mutations).toBe(3);
    expect(
      (
        await call("actions-source", jwt, {
          ...input,
          padding: "x".repeat(4096),
        })
      ).status,
    ).toBe(413);
    expect(reads).toBe(1);
  } finally {
    if (old.token === undefined)
      delete process.env.WEBDOCK_PLUGIN_INSTALLER_TOKEN;
    else process.env.WEBDOCK_PLUGIN_INSTALLER_TOKEN = old.token;
    if (old.file === undefined) delete process.env.WEBDOCK_PLUGIN_RUNTIME_FILE;
    else process.env.WEBDOCK_PLUGIN_RUNTIME_FILE = old.file;
  }
});
