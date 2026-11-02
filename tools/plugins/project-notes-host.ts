/** Owned local authoring host: real Core services and RLS, explicit fixture identities, NOT a production installer. */
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { randomUUID, randomBytes } from "node:crypto";
import { readFile } from "node:fs/promises";
import { parseEnv } from "node:util";
import { resolve } from "node:path";
import { Pool } from "pg";
import {
  ScopedPool,
  withDatabaseScope,
  type DatabaseScope,
} from "@webdock/database-runtime";
import { createPluginData } from "@webdock/core/plugin-data";
import { createPluginJobs, type JobScope } from "@webdock/core/plugin-jobs";
import {
  PluginError,
  parsePluginManifest,
} from "@webdock/plugin-sdk/contracts";
const root = resolve(import.meta.dirname, "../.."),
  directory = resolve(process.argv[2] ?? "");
if (!process.argv[2]) throw Error("Pass built Project Notes directory");
const env = parseEnv(
  await readFile(
    resolve(root, "legacy/control-plane/.env.rls.test.local"),
    "utf8",
  ),
);
const fixture = JSON.parse(
  await readFile(env.WEBDOCK_RLS_FIXTURE_FILE!, "utf8"),
);
const target = new URL(env.API_TEST_OWNER_URL!);
if (target.hostname !== "127.0.0.1" || target.pathname !== "/webdock_rls_test")
  throw Error("Owned local RLS fixture required");
const owner = new Pool({ connectionString: target.href, max: 3 });
const api = new ScopedPool(
  { connectionString: fixture.roles.webdock_auth_runtime, max: 4 },
  { secret: fixture.contextSecret, audience: "webdock_auth_runtime" },
);
const workerDB = new ScopedPool(
  { connectionString: fixture.roles.webdock_worker_runtime, max: 3 },
  { secret: fixture.contextSecret, audience: "webdock_worker_runtime" },
);
const manifest = parsePluginManifest(
  JSON.parse(await readFile(resolve(directory, "tabularium.json"), "utf8")),
);
const plugin = "notes-fixture-" + randomBytes(4).toString("hex");
const base = BigInt(
  "94" + BigInt("0x" + randomBytes(5).toString("hex")).toString(),
);
const id = (i: number) => (base + BigInt(i)).toString();
const customer = id(0),
  user = id(1),
  session = id(2),
  projects = [id(3), id(4)];
const capabilities = manifest.capabilities;
const names = manifest.jobs.map((x) => x.name);
await owner.query(
  "INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES($1,'external','enabled','enabled','0.1.0','ready',1)",
  [plugin],
);
await owner.query(
  "INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities) VALUES($1,$2::jsonb)",
  [plugin, JSON.stringify(capabilities)],
);
await owner.query(
  "INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES($1,'0.1.0',$2,$3,$4,'https://fixture.invalid','fixture')",
  [plugin, JSON.stringify(manifest), "0".repeat(64), manifest.image],
);
await owner.query(
  "INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision) VALUES($1,1,'0.1.0',$2,'ready',0)",
  [plugin, manifest.image],
);
await owner.query(
  'INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword") VALUES($1,\'Notes developer\',$2,\'user\',true,false)',
  [user, user + "@example.invalid"],
);
await owner.query(
  'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval \'8 hours\',now())',
  [session, user],
);
await owner.query(
  "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,'Notes fixture','active')",
  [customer],
);
const org = (
  await owner.query(
    "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
    [customer],
  )
).rows[0].organization_id;
await owner.query(
  'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'member\',now())',
  [org, user],
);
for (const project of projects) {
  await owner.query(
    "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1,$2,$3,'active')",
    [
      project,
      project === projects[0] ? "Website Relaunch" : "Customer Portal",
      customer,
    ],
  );
  await owner.query(
    "INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities,jobs) VALUES($1,$2,$3,$4::jsonb,$5::jsonb)",
    [
      plugin,
      customer,
      project,
      JSON.stringify(capabilities),
      JSON.stringify(names),
    ],
  );
}
type Invocation = {
  dbScope: DatabaseScope;
  scope: JobScope;
  operationID: string;
  expires: number;
};
const tokens = new Map<string, Invocation>();
const asScope = (projectID: string): JobScope => ({
  pluginID: plugin,
  customerID: customer,
  projectID,
  runtimeGeneration: "1",
  grantRevision: "0",
});
async function issue(
  project: string,
  name: string,
  kind: "action" | "job",
  operationID: string,
  jobClaim?: string,
) {
  const declaration =
    kind === "action"
      ? manifest.actions.find((x) => x.name === name)
      : manifest.jobs.find((x) => x.name === name);
  if (!declaration) throw Error("Undeclared handler");
  const invocation = randomUUID(),
    token = randomBytes(32).toString("hex");
  await owner.query(
    "INSERT INTO webdock_auth.plugin_invocation(id,plugin_id,customer_id,project_id,runtime_generation,grant_revision,kind,name,intent,actor_id,session_id,job_claim_token,job_execution_id,operation_id,capabilities,expires_at) VALUES($1,$2,$3,$4,1,0,$5,$6,$7,$8,$9,$10,$11,$12,$13::jsonb,now()+interval '50 seconds')",
    [
      invocation,
      plugin,
      customer,
      project,
      kind,
      name,
      kind === "job" ? "write" : (declaration as any).intent,
      kind === "action" ? user : null,
      kind === "action" ? session : null,
      jobClaim ?? null,
      kind === "job" ? operationID : null,
      operationID,
      JSON.stringify(declaration.capabilities),
    ],
  );
  tokens.set(token, {
    dbScope:
      kind === "action"
        ? {
            kind: "session",
            subject: user,
            sessionID: session,
            pluginInvocationID: invocation,
          }
        : {
            kind: "system",
            purpose: "plugin-job",
            pluginInvocationID: invocation,
          },
    scope: asScope(project),
    operationID,
    expires: Date.now() + 50_000,
  });
  return token;
}
function invocation(req: Request) {
  const token = req.headers.get("authorization")?.replace(/^Bearer /, "") ?? "";
  const found = tokens.get(token);
  if (!found || found.expires < Date.now())
    throw new PluginError("permission_denied", "Invocation expired");
  return found;
}
async function authorize(req: Request, cap: string) {
  const inv = invocation(req);
  const pool = inv.dbScope.kind === "session" ? api : workerDB;
  const result = await withDatabaseScope(inv.dbScope, () =>
    pool.query("SELECT webdock_rls.plugin_context($1) AS context", [cap]),
  );
  if (!result.rows[0].context)
    throw new PluginError(
      "permission_denied",
      "Invocation revoked or capability denied",
    );
  return { inv, pool };
}
const data = createPluginData({
  run: async (req, cap, run) => {
    const { inv, pool } = await authorize(req, cap);
    return withDatabaseScope(inv.dbScope, () =>
      run({ ...inv.scope, operationID: inv.operationID }, pool),
    );
  },
});
let pluginOrigin = "";
const runtimeToken = randomBytes(32).toString("hex");
const jobs = createPluginJobs({
  connection: {
    host: "127.0.0.1",
    port: Number(process.env.WEBDOCK_TEST_REDIS_PORT),
  },
  prefix: "notes-" + randomUUID(),
  authority: {
    authorize: async (req, cap) => ({
      scope: (await authorize(req, cap)).inv.scope,
      jobs: names,
    }),
    canRun: async (scope, name) => {
      const r = await owner.query(
        "SELECT 1 FROM webdock_auth.plugin_installation p JOIN webdock_auth.plugin_grant g ON g.plugin_id=p.id WHERE p.id=$1 AND p.state='enabled' AND p.current_generation=$2 AND g.customer_id=$3 AND g.project_id=$4 AND g.revision=$5 AND g.revoked_at IS NULL AND g.jobs ? $6",
        [
          scope.pluginID,
          scope.runtimeGeneration,
          scope.customerID,
          scope.projectID,
          scope.grantRevision,
          name,
        ],
      );
      return r.rowCount === 1;
    },
  },
  execute: async (scope, name, payload, { jobID, signal }) => {
    const claim = randomUUID();
    await owner.query(
      "INSERT INTO webdock_auth.plugin_job_execution(id,plugin_id,customer_id,project_id,name,payload,grant_revision,runtime_generation,status,claim_token,lease_until) VALUES($1,$2,$3,$4,$5,$6::jsonb,0,1,'running',$7,now()+interval '55 seconds') ON CONFLICT(id) DO UPDATE SET status='running',claim_token=$7,lease_until=now()+interval '55 seconds',cancel_requested=false",
      [
        jobID,
        plugin,
        customer,
        scope.projectID,
        name,
        JSON.stringify(payload),
        claim,
      ],
    );
    const token = await issue(scope.projectID, name, "job", jobID, claim);
    const abort = () => {
      void owner
        .query(
          "UPDATE webdock_auth.plugin_job_execution SET cancel_requested=true WHERE id=$1 AND claim_token=$2",
          [jobID, claim],
        )
        .catch(() => {});
    };
    signal.addEventListener("abort", abort, { once: true });
    try {
      const r = await fetch(pluginOrigin + "/webdock/jobs", {
        method: "POST",
        signal,
        headers: {
          Authorization: "Bearer " + runtimeToken,
          "x-plugin-invocation": token,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ name, payload }),
      });
      if (!r.ok) throw Error("Plugin job failed");
      const result = await r.json();
      await owner.query(
        "UPDATE webdock_auth.plugin_job_execution SET status='succeeded' WHERE id=$1 AND claim_token=$2",
        [jobID, claim],
      );
      return result;
    } catch (e) {
      await owner.query(
        "UPDATE webdock_auth.plugin_job_execution SET status='failed' WHERE id=$1 AND claim_token=$2",
        [jobID, claim],
      );
      throw e;
    } finally {
      signal.removeEventListener("abort", abort);
      tokens.delete(token);
    }
  },
});
const browserToken = randomBytes(32).toString("hex"),
  csrf = randomBytes(24).toString("hex");
let origin = "";
const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url ?? "/", origin || "http://localhost");
    if (req.method === "GET" && url.pathname === "/") {
      res.setHeader(
        "Set-Cookie",
        "notes_dev=" + browserToken + "; HttpOnly; SameSite=Strict; Path=/",
      );
      res.setHeader("Content-Type", "text/html; charset=utf-8");
      res.end(shell());
      return;
    }
    let bytes = 0;
    const chunks = [];
    for await (const chunk of req) {
      bytes += chunk.length;
      if (bytes > 72 * 1024) {
        res.writeHead(413);
        res.end();
        return;
      }
      chunks.push(chunk);
    }
    const request = new Request(url, {
      method: req.method,
      headers: req.headers as Record<string, string>,
      body: chunks.length ? Buffer.concat(chunks) : undefined,
    });
    if (url.pathname.startsWith("/v1/plugin/")) {
      const r = url.pathname.startsWith("/v1/plugin/data")
        ? await data.handle(request)
        : await jobs.handle(request);
      res.writeHead(r.status, Object.fromEntries(r.headers));
      res.end(await r.text());
      return;
    }
    if (url.pathname === "/action" && req.method === "POST") {
      if (
        !req.headers.cookie
          ?.split("; ")
          .includes("notes_dev=" + browserToken) ||
        req.headers.origin !== origin ||
        req.headers["x-csrf"] !== csrf
      ) {
        res.writeHead(403);
        res.end();
        return;
      }
      const { project, name, input } = JSON.parse(
        Buffer.concat(chunks).toString(),
      );
      if (
        !projects.includes(project) ||
        !manifest.actions.some((x) => x.name === name)
      )
        throw Error("Invalid action scope");
      const operationID =
        typeof input?.requestID === "string" &&
        /^[A-Za-z0-9_.-]{8,128}$/.test(input.requestID)
          ? input.requestID
          : randomUUID();
      const token = await issue(project, name, "action", operationID);
      try {
        const r = await fetch(pluginOrigin + "/webdock/actions", {
          method: "POST",
          headers: {
            Authorization: "Bearer " + runtimeToken,
            "x-plugin-invocation": token,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({ name, input }),
        });
        res.writeHead(r.status, { "Content-Type": "application/json" });
        res.end(await r.text());
      } finally {
        tokens.delete(token);
      }
      return;
    }
    res.writeHead(404);
    res.end();
  } catch {
    res.writeHead(400, { "Content-Type": "application/json" });
    res.end('{"error":"Invalid request"}');
  }
});
await new Promise<void>((r) =>
  server.listen(Number(process.env.PORT ?? 0), "127.0.0.1", r),
);
origin = "http://127.0.0.1:" + (server.address() as any).port;
let child!: ReturnType<typeof spawn>;
async function startPlugin() {
  child = spawn(process.execPath, ["dist/index.js"], {
    cwd: directory,
    env: {
      ...process.env,
      PORT: "0",
      HOST: "127.0.0.1",
      WEBDOCK_PLUGIN_HOST_URL: origin,
      WEBDOCK_PLUGIN_RUNTIME_TOKEN: runtimeToken,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr!.on("data", (c) => process.stderr.write(c));
  pluginOrigin =
    "http://127.0.0.1:" +
    (await new Promise<number>((resolve, reject) => {
      let buffer = "";
      const timer = setTimeout(
        () => reject(Error("Plugin startup timeout")),
        10000,
      );
      child.once("exit", (code) => reject(Error("Plugin exited " + code)));
      child.stdout!.on("data", (c) => {
        buffer += c;
        try {
          const parsed = JSON.parse(buffer.split("\n")[0]);
          if (parsed.port) {
            clearTimeout(timer);
            resolve(parsed.port);
          }
        } catch {}
      });
    }));
}
await startPlugin();
const worker = jobs.startWorker();
await worker.waitUntilReady();
for (const project of projects)
  await jobs.schedule(asScope(project), "notes.reconcile", 60);
console.log(
  JSON.stringify({
    url: origin,
    pluginOrigin,
    projects,
    fixturePluginID: plugin,
    mode: "local-authoring",
    backend: "PostgreSQL RLS + Core BullMQ",
  }),
);
let closing = false;
async function close() {
  if (closing) return;
  closing = true;
  await worker.close();
  child.kill("SIGTERM");
  await jobs.close();
  server.closeAllConnections();
  server.close();
  await api.end();
  await workerDB.end();
  await owner.end();
}
process.on("SIGTERM", () => void close());
process.on("SIGINT", () => void close());
if (process.argv.includes("--smoke")) {
  const call = async (project: string, name: string, input: unknown) => {
    const r = await fetch(origin + "/action", {
      method: "POST",
      headers: {
        Cookie: "notes_dev=" + browserToken,
        Origin: origin,
        "X-CSRF": csrf,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ project, name, input }),
    });
    return { status: r.status, data: (await r.json()) as any };
  };
  try {
    const dataToken = await issue(projects[0], "notes.save", "action", randomUUID());
    const writeProbe = async (value: unknown, expectedRevision: string | null, idempotencyKey: string) => {
      const response = await fetch(origin + "/v1/plugin/data", {method:"PUT",headers:{Authorization:"Bearer " + dataToken,"Content-Type":"application/json"},body:JSON.stringify({key:"contract-probe",value,expectedRevision,idempotencyKey})});
      return {status:response.status,data:await response.json()};
    };
    const firstWrite = await writeProbe({probe:1}, null, "probe-write-1");
    assert.equal(firstWrite.status, 200);
    assert.deepEqual(await writeProbe({probe:1}, null, "probe-write-1"), firstWrite);
    assert.equal((await writeProbe({probe:2}, null, "probe-write-1")).status, 409);
    const racing = await Promise.all([writeProbe({probe:2}, "1", "probe-race-a"),writeProbe({probe:3}, "1", "probe-race-b")]);
    assert.deepEqual(racing.map(x=>x.status).sort(), [200,409]);
    tokens.delete(dataToken);
    const noteID = randomUUID();
    const input = {
      id: noteID,
      title: "Launch checklist",
      body: "Verify DNS and backups",
      reminderAt: new Date(Date.now() + 2500).toISOString(),
      expectedRevision: null,
      requestID: randomUUID(),
    };
    const saved = await call(projects[0], "notes.save", input);
    assert.equal(saved.status, 200);
    assert.equal(saved.data.notes.length, 1);
    const other = await call(projects[1], "notes.list", {});
    assert.deepEqual(other.data.notes, []);
    const stale = await call(projects[0], "notes.save", {
      ...input,
      title: "Stale",
      requestID: randomUUID(),
    });
    assert.notEqual(stale.status, 200);
    const retry = await call(projects[0], "notes.save", input);
    assert.equal(retry.status, 200);
    assert.equal(retry.data.notes.length, 1);
    let latest = saved;
    for (let i = 0; i < 100; i++) {
      latest = await call(projects[0], "notes.list", {});
      if (latest.data.notes[0]?.reminderState === "due") break;
      await new Promise((r) => setTimeout(r, 100));
    }
    assert.equal(latest.data.notes[0].reminderState, "due");
    child.kill("SIGTERM");
    await new Promise<void>((r) => child.once("exit", () => r()));
    await startPlugin();
    latest = await call(projects[0], "notes.list", {});
    assert.equal(latest.data.notes[0].title, "Launch checklist");
    const deleted = await call(projects[0], "notes.delete", {
      id: noteID,
      expectedRevision: latest.data.revision,
      requestID: randomUUID(),
    });
    assert.equal(deleted.status, 200);
    assert.deepEqual(deleted.data.notes, []);
    await owner.query(
      "UPDATE webdock_auth.plugin_grant SET revoked_at=now() WHERE plugin_id=$1 AND project_id=$2",
      [plugin, projects[0]],
    );
    assert.notEqual((await call(projects[0], "notes.list", {})).status, 200);
    const persisted = (
      await owner.query(
        "SELECT count(*)::int AS n FROM webdock_auth.plugin_document_receipt WHERE plugin_id=$1",
        [plugin],
      )
    ).rows[0].n;
    assert.ok(persisted >= 3);
    console.log(
      JSON.stringify({
        projectNotes: true,
        postgresRLS: true,
        projectIsolation: true,
        staleWriteRejected: true,
        idempotentSave: true,
        bullmqReminder: true,
        separatePluginRestartRetainsData: true,
        delete: true,
        revokedGrantDenied: true,
        receipts: persisted,
        coreDocumentReplay: true,
        coreConcurrentCAS: true,
      }),
    );
  } finally {
    await close();
  }
}
function shell() {
  return `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Project Notes · local Webdock preview</title><style>body{margin:0;background:#e9edf1;color:#1b293c;font:15px system-ui}header{padding:20px 28px;display:flex;justify-content:space-between;align-items:center;background:#1b293c;color:white}header strong{font-size:22px}header span{color:#ff9255}main{max-width:1080px;margin:28px auto;padding:0 16px}select{padding:10px;margin:14px 0 18px;border:1px solid #b7c1cb;border-radius:5px;font:inherit}iframe{width:100%;height:720px;border:1px solid #cbd2d9;border-radius:8px;background:white}p{color:#536477}.label{font-size:12px;letter-spacing:.06em}</style><header><strong>webdock<span>.</span></strong><span class="label">LOCAL DEVELOPER PREVIEW</span></header><main><p>Project Notes · PostgreSQL with RLS · BullMQ · isolated plugin panel</p><label for="project">Project</label> <select id="project"><option value="${projects[0]}">Website Relaunch</option><option value="${projects[1]}">Customer Portal</option></select><div id="slot"></div></main><script>let channel;function mount(){channel?.port1.close();const project=document.getElementById('project').value,nonce=crypto.randomUUID();const frame=document.createElement('iframe');frame.title='Project Notes';frame.sandbox='allow-scripts';frame.src=${JSON.stringify(pluginOrigin)}+'/ui/notes#'+nonce;document.getElementById('slot').replaceChildren(frame);frame.onload=()=>{channel=new MessageChannel();const port=channel.port1;port.onmessage=async e=>{const m=e.data;if(!m||m.type!=='action'||typeof m.id!=='string'||JSON.stringify(m).length>32768)return;try{const r=await fetch('/action',{method:'POST',headers:{'Content-Type':'application/json','X-CSRF':${JSON.stringify(csrf)}},body:JSON.stringify({project,name:m.name,input:m.input})});const result=await r.json();port.postMessage(r.ok?{id:m.id,result}:{id:m.id,error:result.error??'Request failed'});}catch{port.postMessage({id:m.id,error:'Host unavailable'});}};frame.contentWindow.postMessage({type:'webdock:init',nonce,projectName:document.getElementById('project').selectedOptions[0].text},'*',[channel.port2]);};}document.getElementById('project').onchange=mount;mount();</script></html>`;
}
