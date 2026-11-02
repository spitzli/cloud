/** Disposable local RLS + real TLS proof for the production runtime adapter. */
import { readFileSync, mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseEnv } from "node:util";
import {
  createHash,
  generateKeyPairSync,
  randomBytes,
  randomUUID,
} from "node:crypto";
import { execFileSync } from "node:child_process";
import assert from "node:assert/strict";
import { Pool } from "pg";
import { withDatabaseScope } from "@webdock/database-runtime";
import {
  createPluginContext,
  verifyInvocation,
} from "@webdock/plugin-sdk/server";
const env = parseEnv(
  readFileSync(process.env.WEBDOCK_PLUGIN_TEST_ENV ?? "legacy/control-plane/.env.rls.test.local", "utf8"),
);
const u = new URL(env.API_TEST_OWNER_URL!);
if (u.hostname !== "127.0.0.1" || u.pathname !== "/webdock_rls_test")
  throw Error("Isolated fixture required");
const fixture = JSON.parse(readFileSync(env.WEBDOCK_RLS_FIXTURE_FILE!, "utf8"));
Object.assign(process.env, {
  NODE_ENV: "development",
  DATABASE_URL: fixture.roles.webdock_auth_runtime,
  AUTH_DATABASE_URL: fixture.roles.webdock_auth_runtime,
  DATABASE_WORKER_URL: fixture.roles.webdock_worker_runtime,
  DATABASE_RLS_SECRET: fixture.contextSecret,
  DATABASE_POOL_MAX: "3",
});
const owner = new Pool({ connectionString: u.href, max: 1 });
const dir = mkdtempSync(join(tmpdir(), "webdock-plugin-runtime-"));
const plugin = "runtime-" + randomBytes(4).toString("hex");
const project = (BigInt("95" + Date.now()) + 1n).toString(),
  customer = (BigInt(project) + 1n).toString(),
  foreign = (BigInt(project) + 2n).toString();
const subject = randomUUID(),
  session = randomUUID();
const credential = randomBytes(32).toString("hex");
execFileSync(
  "openssl",
  [
    "req",
    "-x509",
    "-newkey",
    "rsa:2048",
    "-nodes",
    "-keyout",
    join(dir, "key.pem"),
    "-out",
    join(dir, "cert.pem"),
    "-days",
    "1",
    "-subj",
    "/CN=localhost",
    "-addext",
    "subjectAltName=DNS:localhost,DNS:fixture.plugin-test.svc.cluster.local",
  ],
  { stdio: "ignore" },
);
const cert = readFileSync(join(dir, "cert.pem"), "utf8"),
  key = readFileSync(join(dir, "key.pem"), "utf8");
const pair = generateKeyPairSync("ed25519");
const publicKey = pair.publicKey
  .export({ type: "spki", format: "pem" })
  .toString();
const configuration = {
  privateKey: pair.privateKey
    .export({ type: "pkcs8", format: "pem" })
    .toString(),
  hostOrigin: "https://localhost:3444",
  runtimes: {
    [plugin]: {
      generation: 1,
      namespace: "plugin-test",
      service: "fixture",
      addresses: ["127.0.0.1"],
      credential,
      ca: cert,
      projects: {
        [project]: {
          provider: {
            origin: "https://localhost:3445",
            addresses: ["127.0.0.1"],
            ca: cert,
            headers: { authorization: "Bearer provider-secret" },
            rules: [{ method: "GET", path: "/stats", query: [] }],
          },
        },
      },
    },
  },
};
writeFileSync(join(dir, "runtime.json"), JSON.stringify(configuration), {
  mode: 0o600,
});
process.env.WEBDOCK_PLUGIN_RUNTIME_FILE = join(dir, "runtime.json");
const runtime =
  await import("../../legacy/control-plane/src/modules/plugins/runtime");
const manifest = JSON.parse(
  readFileSync("tools/plugins/fixtures/project-notes.json", "utf8"),
);
Object.assign(manifest, {
  name: plugin,
  version: "1.0.0",
  capabilities: ["plugin.data.read", "plugin.data.write", "plugin.http.read"],
  httpBindings: ["provider"],
  actions: [
    { name: "noop", intent: "read", capabilities: [] },
    {
      name: "save",
      intent: "write",
      capabilities: [
        "plugin.data.read",
        "plugin.data.write",
        "plugin.http.read",
      ],
    },
  ],
  api: [
    { method: "GET", path: "/noop", action: "noop" },
    { method: "POST", path: "/save", action: "save" },
  ],
  mcp: [
    { name: "noop", description: "Read fixture", action: "noop" },
    { name: "save", description: "Write fixture", action: "save" },
  ],
  jobs: [],
  slots: [],
  navigation: [],
  events: [],
});
const raw = JSON.stringify(manifest);
let savedToken = "",
  revokedDuring = false;
const provider = Bun.serve({
  hostname: "127.0.0.1",
  port: 3445,
  tls: { cert, key },
  fetch: (request) =>
    Response.json({
      authorized:
        request.headers.get("authorization") === "Bearer provider-secret",
    }),
});
const host = Bun.serve({
  hostname: "127.0.0.1",
  port: 3444,
  tls: { cert, key },
  async fetch(request) {
    try {
      if (new URL(request.url).pathname === "/v1/plugin/http")
        return Response.json(
          await runtime.pluginHTTP.request(request, await request.json()),
        );
      return await runtime.pluginData.handle(request);
    } catch {
      return Response.json(
        { error: { code: "permission_denied", message: "Denied" } },
        { status: 403 },
      );
    }
  },
});
const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 3443,
  tls: { cert, key },
  async fetch(request) {
    try {
      const token = request.headers.get("x-plugin-invocation")!;
      savedToken = token;
      verifyInvocation(token, { publicKey, pluginID: plugin, generation: 1 });
      const b: any = await request.json();
      if (new URL(request.url).pathname.endsWith("/noop"))
        return Response.json({ ok: true });
      const ctx = createPluginContext({
        hostURL: "https://localhost:3444",
        token: () => token,
        runtimeCredential: credential,
        fetch: ((url: any, options: any) =>
          fetch(url, { ...options, tls: { ca: cert } })) as typeof fetch,
      });
      const data = await ctx.data.read("state");
      const upstream = await ctx.http.request({
        binding: "provider",
        method: "GET",
        path: "/stats",
      });
      await ctx.data.write({
        key: "state",
        value: b.input,
        expectedRevision: data.revision,
        idempotencyKey: "save-state",
      });
      if (revokedDuring)
        await owner.query(
          "UPDATE webdock_auth.plugin_grant SET revoked_at=now() WHERE plugin_id=$1",
          [plugin],
        );
      return Response.json({ saved: b.input, provider: upstream.body });
    } catch (e) {
      return Response.json({ error: String(e) }, { status: 400 });
    }
  },
});
const run = <T>(fn: () => Promise<T>) =>
  withDatabaseScope({ kind: "session", subject, sessionID: session }, fn);
try {
  await owner.query(
    `INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword") VALUES($1,'Runtime test',$2,'user',true,false)`,
    [subject, subject + "@example.invalid"],
  );
  await owner.query(
    `INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval '1 hour',now())`,
    [session, subject],
  );
  for (const id of [customer, foreign])
    await owner.query(
      "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,'Runtime fixture','active')",
      [id],
    );
  const org = (
    await owner.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customer],
    )
  ).rows[0].organization_id;
  await owner.query(
    `INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,'admin',now())`,
    [org, subject],
  );
  for (const [id, c] of [
    [project, customer],
    [foreign, foreign],
  ])
    await owner.query(
      "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1,'Runtime fixture',$2,'active')",
      [id, c],
    );
  await owner.query(
    "INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES($1,'external','enabled','enabled','1.0.0','ready',1)",
    [plugin],
  );
  await owner.query(
    "INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES($1,'1.0.0',$2,$3,$4,'https://fixture.invalid','fixture')",
    [
      plugin,
      raw,
      createHash("sha256").update(raw).digest("hex"),
      manifest.image,
    ],
  );
  await owner.query(
    "INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities) VALUES($1,$2::jsonb)",
    [plugin, JSON.stringify(manifest.capabilities)],
  );
  await owner.query(
    "INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,namespace,service_name,credential_hash) VALUES($1,1,'1.0.0',$2,'ready',0,'plugin-test','fixture',$3)",
    [
      plugin,
      manifest.image,
      createHash("sha256").update(credential).digest("hex"),
    ],
  );
  await owner.query(
    "INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities) VALUES($1,$2,$3,$4::jsonb)",
    [plugin, customer, project, JSON.stringify(manifest.capabilities)],
  );
  const input = {
    pluginID: plugin,
    projectID: project,
    action: "save",
    input: { message: "real TLS and RLS" },
    idempotencyKey: "first",
  };
  assert.deepEqual(await run(() => runtime.invokeRuntimePlugin(input)), {
    saved: input.input,
    provider: { authorized: true },
  });
  await assert.rejects(
    () =>
      run(() =>
        runtime.invokeRuntimePlugin(
          { ...input, idempotencyKey: "readonly-token" },
          { canWrite: false },
        ),
      ),
    /Write scope/,
  );
  assert.deepEqual(
    await run(() =>
      runtime.invokeRuntimePlugin(
        { ...input, action: "noop", input: {}, idempotencyKey: "noop" },
        { canWrite: false },
      ),
    ),
    { ok: true },
  );
  const { McpServer } = await import("@modelcontextprotocol/sdk/server/mcp.js");
  const { Client } = await import("@modelcontextprotocol/sdk/client/index.js");
  const { InMemoryTransport } =
    await import("@modelcontextprotocol/sdk/inMemory.js");
  const { registerPluginTools } =
    await import("../../legacy/control-plane/src/modules/plugins/exposure");
  const mcp = new McpServer({ name: "fixture", version: "1" });
  await registerPluginTools(mcp, false, async () => [
    await runtime.getRuntimePlugin(plugin),
  ]);
  const [clientTransport, serverTransport] =
    InMemoryTransport.createLinkedPair();
  const client = new Client({ name: "fixture-client", version: "1" });
  await mcp.connect(serverTransport);
  await client.connect(clientTransport);
  const tools = await client.listTools();
  assert.deepEqual(
    tools.tools.map((t) => t.name),
    ["plugin__" + plugin.replaceAll("-", "_") + "__noop"],
  );
  const mcpResult = await run(() =>
    client.callTool({
      name: tools.tools[0].name,
      arguments: { projectID: project, input: {} },
    }),
  );
  assert.equal(mcpResult.isError, undefined);
  await client.close();
  await mcp.close();
  const { pluginProtocol } =
    await import("../../legacy/control-plane/src/protocols/plugins");
  const api = pluginProtocol(
    {
      production: false,
      apiOrigin: "https://api.fixture.test",
      authOrigin: "https://auth.fixture.test",
      studioOrigin: "https://studio.fixture.test",
      port: 3130,
      domains: [],
    },
    (async (_request: any, write: boolean) =>
      write
        ? new Response(null, { status: 403 })
        : {
            actor: { identity: { subject, sessionID: session } },
            canWrite: false,
          }) as any,
  ).app;
  assert.equal(
    (
      await api.handle(
        new Request(
          "https://api.fixture.test/v1/plugins/" +
            plugin +
            "/projects/" +
            project +
            "/api/noop",
        ),
      )
    ).status,
    200,
  );
  assert.equal(
    (
      await api.handle(
        new Request(
          "https://api.fixture.test/v1/plugins/" +
            plugin +
            "/projects/" +
            project +
            "/api/save",
          {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: "{}",
          },
        ),
      )
    ).status,
    403,
  );
  await assert.rejects(
    () =>
      run(() =>
        runtime.invokeRuntimePlugin({
          ...input,
          input: { message: "changed" },
        }),
      ),
    /Idempotency/,
  );
  await assert.rejects(() =>
    run(() => runtime.invokeRuntimePlugin({ ...input, projectID: foreign })),
  );
  assert.equal(
    (
      await runtime.pluginData.handle(
        new Request("https://localhost:3444/v1/plugin/data/state", {
          headers: {
            authorization: "Bearer " + savedToken,
            "x-plugin-runtime": credential,
          },
        }),
      )
    ).status,
    403,
  );
  await owner.query(
    'UPDATE webdock_auth.member SET role=\'member\' WHERE "organizationId"=$1 AND "userId"=$2',
    [org, subject],
  );
  await assert.rejects(() =>
    run(() =>
      runtime.invokeRuntimePlugin({ ...input, idempotencyKey: "member" }),
    ),
  );
  await owner.query(
    'UPDATE webdock_auth.member SET role=\'admin\' WHERE "organizationId"=$1 AND "userId"=$2',
    [org, subject],
  );
  revokedDuring = true;
  await assert.rejects(() =>
    run(() =>
      runtime.invokeRuntimePlugin({ ...input, idempotencyKey: "revoked" }),
    ),
  );
  console.log(
    JSON.stringify({
      realTLS: true,
      scopedData: true,
      providerCredentialHidden: true,
      crossProjectDenied: true,
      readOnlyMembershipDenied: true,
      revokedGrantDenied: true,
      idempotencyConflictDenied: true,
    }),
  );
} catch (error) {
  console.error(error);
  throw error;
} finally {
  server.stop(true);
  host.stop(true);
  provider.stop(true);
  for (const table of [
    "plugin_document_receipt",
    "plugin_document",
    "plugin_invocation",
    "plugin_job_execution",
    "plugin_grant",
    "plugin_runtime",
    "plugin_configuration",
    "plugin_release",
    "plugin_operation",
  ])
    await owner.query(`DELETE FROM webdock_auth.${table} WHERE plugin_id=$1`, [
      plugin,
    ]);
  await owner.query(
    "DELETE FROM webdock_auth.plugin_installation WHERE id=$1",
    [plugin],
  );
  await owner.query(
    "DELETE FROM webdock_admin.projects WHERE id=ANY($1::text[])",
    [[project, foreign]],
  );
  await owner.query(
    "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1::text[])",
    [[customer, foreign]],
  );
  await owner.query(
    "DELETE FROM webdock_admin.customers WHERE id=ANY($1::text[])",
    [[customer, foreign]],
  );
  await owner.query("DELETE FROM webdock_auth.session WHERE id=$1", [session]);
  await owner.query('DELETE FROM webdock_auth.member WHERE "userId"=$1', [
    subject,
  ]);
  await owner.query('DELETE FROM webdock_auth."user" WHERE id=$1', [subject]);
  await owner.end();
  await (
    await import("../../legacy/control-plane/src/db/runtime")
  ).closeDatabases();
  rmSync(dir, { recursive: true, force: true });
}
