import { test, expect } from "bun:test";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
const file = new URL(
  "../src/modules/plugins/sources-policy.ts",
  import.meta.url,
);
const policy = existsSync(file) ? await import(file.pathname) : null;
const serviceFile = new URL(
  "../src/modules/plugins/sources-store.ts",
  import.meta.url,
);
const service = existsSync(serviceFile)
  ? await import(serviceFile.pathname)
  : null;
const runtimeFile = new URL(
  "../src/modules/plugins/sources-runtime.ts",
  import.meta.url,
);
const runtime = existsSync(runtimeFile)
  ? await import(runtimeFile.pathname)
  : null;
const identityFile = new URL(
  "../src/modules/plugins/sources-identity.ts",
  import.meta.url,
);
const identity = existsSync(identityFile)
  ? await import(identityFile.pathname)
  : null;
test("source ownership uses Auth identity while GitHub retains the distinct Registry profile identity", () => {
  expect(identity).not.toBeNull();
  const scope = {
    kind: "session",
    subject: "auth-user-distinct",
    sessionID: "verified-session",
  };
  const actor = {
    identity: { subject: "auth-user-distinct", sessionID: "verified-session" },
    user: { id: "registry-profile-distinct" },
  };
  const resolved = identity.sourceOperatorIdentity(actor, scope);
  expect(resolved.id).toBe("auth-user-distinct");
  expect(resolved.registryID).toBe("registry-profile-distinct");
  expect(resolved.scope).toEqual(scope);
  expect(
    identity.sourceOperatorIdentity(actor, scope, "auth-user-distinct")
      .registryID,
  ).toBe("registry-profile-distinct");
  expect(() =>
    identity.sourceOperatorIdentity(actor, scope, "registry-profile-distinct"),
  ).toThrow();
  expect(() =>
    identity.sourceOperatorIdentity(
      { ...actor, identity: { ...actor.identity, sessionID: "other-session" } },
      scope,
    ),
  ).toThrow();
  expect(() =>
    identity.sourceOperatorIdentity(
      { ...actor, identity: { ...actor.identity, subject: "other-auth-user" } },
      scope,
    ),
  ).toThrow();
});
test("activation waits for a missing mounted binding but rejects mismatched identity", async () => {
  expect(runtime).not.toBeNull();
  const dir = await mkdtemp(join(tmpdir(), "plugin-source-binding-")),
    file = join(dir, "runtime.json");
  const manifest = JSON.parse(
    await readFile(
      new URL(
        "../../../tools/plugins/fixtures/project-notes.json",
        import.meta.url,
      ),
      "utf8",
    ),
  );
  manifest.jobs = [];
  const row = { manifest_raw: JSON.stringify(manifest) };
  const binding = {
    namespace: "plugins",
    service: "example",
    addresses: ["127.0.0.1"],
    credential: "r".repeat(40),
    ca: "fixture-ca",
    generation: 1,
    projects: {},
  };
  let calls = 0;
  const transport = async (input: any) => {
    calls++;
    expect(input.url.hostname).toBe("example.plugins.svc.cluster.local");
    expect(input.headers.authorization).toBe("Bearer " + binding.credential);
    return {
      status: 200,
      body: {
        pluginID: manifest.name,
        version: manifest.version,
        generation: 1,
        protocolVersion: 1,
      },
    };
  };
  try {
    await writeFile(file, JSON.stringify({ runtimes: {} }), { mode: 0o600 });
    await expect(
      runtime.verifySourceRuntime(row, binding, file, transport),
    ).rejects.toMatchObject({ status: 503, code: "runtime_not_ready" });
    expect(calls).toBe(0);
    await writeFile(
      file,
      JSON.stringify({
        runtimes: { [manifest.name]: { ...binding, generation: 2 } },
      }),
    );
    await expect(
      runtime.verifySourceRuntime(row, binding, file, transport),
    ).rejects.toMatchObject({ status: 409, code: "runtime_binding_changed" });
    expect(calls).toBe(0);
    await writeFile(
      file,
      JSON.stringify({ runtimes: { [manifest.name]: binding } }),
    );
    await runtime.verifySourceRuntime(row, binding, file, transport);
    expect(calls).toBe(1);
    await expect(
      runtime.verifySourceRuntime(row, binding, file, async () => ({
        status: 503,
        body: null,
      })),
    ).rejects.toMatchObject({ status: 503, code: "runtime_not_ready" });
    await expect(
      runtime.verifySourceRuntime(row, binding, file, async () => ({
        status: 200,
        body: {
          pluginID: "wrong",
          version: manifest.version,
          generation: 1,
          protocolVersion: 1,
        },
      })),
    ).rejects.toMatchObject({ status: 409, code: "runtime_identity_mismatch" });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test("source credentials are purpose encrypted and never part of public status", () => {
  expect(policy).not.toBeNull();
  const token = "a".repeat(40);
  const sealed = policy.sealSourceSecret(
    { token: "private-token" },
    token,
    "job-1",
  );
  expect(sealed).not.toContain("private-token");
  expect(policy.openSourceSecret(sealed, token, "job-1")).toEqual({
    token: "private-token",
  });
  expect(() => policy.openSourceSecret(sealed, token, "job-2")).toThrow();
  expect(() =>
    policy.openSourceSecret(sealed, "b".repeat(40), "job-1"),
  ).toThrow();
  const view = policy.sourceView({
    id: "1",
    project_id: "2",
    source_kind: "git",
    source_label: "https://example.com/repo",
    status: "queued",
    mode: "build",
    uploaded_bytes: 0,
    expected_bytes: 0,
    created_at: new Date(0),
    secret_cipher: sealed,
    actor_scope: { secret: "private" },
    runtime_cipher: sealed,
  });
  expect(JSON.stringify(view)).not.toContain(sealed);
  expect(JSON.stringify(view)).not.toContain("actor_scope");
});
test("source commands bound uploads and reject credential URLs and unsupported modes", () => {
  expect(policy).not.toBeNull();
  for (const source of [
    { kind: "zip", name: "x.zip", size: 33554433 },
    { kind: "git", url: "https://token@example.com/repo" },
    { kind: "git", url: "http://example.com/repo" },
    { kind: "git", url: "https://example.com/repo?token=secret" },
  ])
    expect(() =>
      policy.parseSourceCommand({
        action: "create",
        projectID: "1",
        source,
        mode: "build",
        idempotencyKey: "x",
      }),
    ).toThrow();
  expect(() =>
    policy.parseSourceCommand({
      action: "create",
      projectID: "1",
      source: { kind: "zip", name: "x.zip", size: 1 },
      mode: "prebuilt",
      idempotencyKey: "x",
    }),
  ).toThrow();
  expect(
    policy
      .decodeSourceChunk(Buffer.from("hello").toString("base64"))
      .toString(),
  ).toBe("hello");
  for (const data of ["", "a===", "Zg", Buffer.alloc(65537).toString("base64")])
    expect(() => policy.decodeSourceChunk(data)).toThrow();
});
test("worker authentication is exact, configured, and fails closed", () => {
  expect(policy).not.toBeNull();
  const env = {
    WEBDOCK_PLUGIN_INSTALLER_TOKEN: "a".repeat(40),
    WEBDOCK_PLUGIN_RUNTIME_FILE: "/runtime.json",
  };
  expect(
    policy.installerAuthorized(
      "Bearer " + env.WEBDOCK_PLUGIN_INSTALLER_TOKEN,
      env,
    ),
  ).toBe(true);
  for (const auth of [
    null,
    "bearer " + env.WEBDOCK_PLUGIN_INSTALLER_TOKEN,
    "Bearer wrong",
    "Bearer " + env.WEBDOCK_PLUGIN_INSTALLER_TOKEN + " ",
  ])
    expect(policy.installerAuthorized(auth, env)).toBe(false);
  expect(
    policy.installerAuthorized("Bearer " + env.WEBDOCK_PLUGIN_INSTALLER_TOKEN, {
      ...env,
      WEBDOCK_PLUGIN_RUNTIME_FILE: "",
    }),
  ).toBe(false);
});
test("expired or stolen worker leases cannot complete or read source bytes", () => {
  expect(policy).not.toBeNull();
  const row = {
    status: "building",
    claim_token: "claim",
    lease_until: new Date(Date.now() + 60000),
    expires_at: new Date(Date.now() + 60000),
  };
  expect(() => policy.requireSourceLease(row, "claim")).not.toThrow();
  for (const changed of [
    { ...row, claim_token: "other" },
    { ...row, lease_until: new Date(0) },
    { ...row, status: "review" },
    { ...row, expires_at: new Date(0) },
  ])
    expect(() => policy.requireSourceLease(changed, "claim")).toThrow();
});
test("upload transaction rejects out-of-order bytes and returns identical retries without writing", async () => {
  expect(service).not.toBeNull();
  const row = {
    id: "10",
    actor_id: "1",
    project_id: "2",
    customer_id: "3",
    status: "uploading",
    source_kind: "zip",
    source_label: "source.zip",
    mode: "build",
    expected_bytes: 10,
    uploaded_bytes: 5,
    created_at: new Date(),
    expires_at: new Date(Date.now() + 60000),
  };
  const writes: string[] = [];
  const client = {
    release() {},
    async query(sql: string) {
      if (sql.includes("webdock_rls.operator()"))
        return { rows: [{ allowed: true }] };
      if (sql.includes("FROM webdock_admin.projects"))
        return { rows: [{ customer_id: "3" }] };
      if (sql.startsWith("SELECT * FROM webdock_auth.plugin_source_import"))
        return { rows: [row] };
      if (sql.includes("SELECT data FROM"))
        return { rows: [{ data: Buffer.from("hello") }] };
      if (sql.startsWith("INSERT") || sql.includes("uploaded_bytes="))
        writes.push(sql);
      return { rows: [], rowCount: 0 };
    },
  };
  const api = service.createPluginSourceService({
    db: { connect: async () => client },
    token: () => "a".repeat(40),
    configured: () => true,
    actor: async () => ({
      id: "1",
      scope: { kind: "session", subject: "1", sessionID: "session" },
    }),
  });
  await expect(
    api.command({
      action: "chunk",
      id: "10",
      offset: 7,
      data: Buffer.from("hello").toString("base64"),
    }),
  ).rejects.toThrow();
  expect(writes).toEqual([]);
  expect(
    (
      await api.command({
        action: "chunk",
        id: "10",
        offset: 0,
        data: Buffer.from("hello").toString("base64"),
      })
    ).uploadedBytes,
  ).toBe(5);
  expect(writes).toEqual([]);
  await expect(
    api.command({
      action: "chunk",
      id: "10",
      offset: 0,
      data: Buffer.from("other").toString("base64"),
    }),
  ).rejects.toThrow();
});
test("revoked operator and expired worker never obtain private source data", async () => {
  expect(service).not.toBeNull();
  let chunkReads = 0;
  const row = {
    id: "10",
    status: "building",
    claim_token: "0d76e81f-f077-4b4d-a8c8-c5da2cb87293",
    lease_until: new Date(0),
    expires_at: new Date(Date.now() + 60000),
  };
  const client = {
    release() {},
    async query(sql: string) {
      if (sql.includes("webdock_rls.operator()"))
        return { rows: [{ allowed: false }] };
      if (sql.startsWith("SELECT * FROM webdock_auth.plugin_source_import"))
        return { rows: [row] };
      if (sql.includes("SELECT data FROM")) chunkReads++;
      return { rows: [], rowCount: 0 };
    },
  };
  const api = service.createPluginSourceService({
    db: { connect: async () => client },
    token: () => "a".repeat(40),
    configured: () => true,
    actor: async () => ({
      id: "1",
      scope: { kind: "session", subject: "1", sessionID: "session" },
    }),
  });
  await expect(
    api.command({ action: "list", projectID: "2" }),
  ).rejects.toThrow();
  await expect(
    api.worker("source", { id: "10", claimToken: row.claim_token, offset: 0 }),
  ).rejects.toThrow();
  expect(chunkReads).toBe(0);
});
