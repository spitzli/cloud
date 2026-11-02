import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { createServer } from "node:http";
import { once } from "node:events";
import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createManagedRuntimeManager } from "../src/managed-runtimes";
import { createGateway } from "../src/server";

// Explicit opt-in: this creates only a disposable loopback Docker database with synthetic credentials.
test("real managed process negotiates, uses verified PostgreSQL TLS, edits schema and erases session state", {
  skip: !process.env.WEBDOCK_MANAGED_RUNTIME_TEST_BINARY && !process.env.WEBDOCK_MANAGED_RUNTIME_TEST_IMAGE && !process.env.WEBDOCK_MANAGED_RUNTIME_TEST_GATEWAY_BINARY, timeout: 60_000,
}, async t => {
  const directory = await mkdtemp(join(tmpdir(), "managed-real-test-"));
  const container = "webdock-managed-viewer-test-" + randomUUID();
  t.after(async () => { try { execFileSync("docker", ["rm", "-f", container], { stdio: "ignore" }); } catch {} await rm(directory, { recursive: true, force: true }); });
  execFileSync("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", join(directory, "ca.key"), "-out", join(directory, "ca.crt"), "-subj", "/CN=fixture-ca", "-days", "1"], { stdio: "ignore" });
  execFileSync("openssl", ["req", "-new", "-newkey", "rsa:2048", "-nodes", "-keyout", join(directory, "server.key"), "-out", join(directory, "server.csr"), "-subj", "/CN=localhost"], { stdio: "ignore" });
  await writeFile(join(directory, "server.ext"), "subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n");
  execFileSync("openssl", ["x509", "-req", "-in", join(directory, "server.csr"), "-CA", join(directory, "ca.crt"), "-CAkey", join(directory, "ca.key"), "-CAcreateserial", "-out", join(directory, "server.crt"), "-extfile", join(directory, "server.ext"), "-days", "1"], { stdio: "ignore" });
  execFileSync("docker", ["run", "--detach", "--rm", "--name", container, "-p", "127.0.0.1::5432", "-v", directory + ":/fixture:ro", "-e", "POSTGRES_PASSWORD=fixture-admin", "--entrypoint", "sh", "postgres:18-alpine", "-c", "cp /fixture/server.key /tmp/server.key; cp /fixture/server.crt /tmp/server.crt; chown postgres:postgres /tmp/server.key /tmp/server.crt; chmod 600 /tmp/server.key; exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/tmp/server.crt -c ssl_key_file=/tmp/server.key"], { stdio: "ignore" });
  let ready = false;
  for (let i = 0; i < 100 && !ready; i++) {
    try { execFileSync("docker", ["exec", container, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"], { stdio: "ignore" }); ready = true; }
    catch { await new Promise(resolve => setTimeout(resolve, 100)); }
  }
  assert.ok(ready);
  execFileSync("docker", ["exec", "-i", container, "psql", "-U", "postgres", "-v", "ON_ERROR_STOP=1"], { input: "CREATE ROLE tenant LOGIN PASSWORD 'managed-fixture-password'; CREATE DATABASE tenant OWNER tenant;", stdio: ["pipe", "ignore", "pipe"] });
  const port = Number(execFileSync("docker", ["port", container, "5432/tcp"], { encoding: "utf8" }).trim().split(":").pop());
  const manager = createManagedRuntimeManager({ executable: process.env.WEBDOCK_MANAGED_RUNTIME_TEST_BINARY || "/usr/local/bin/tabularis", temporaryDirectory: directory, capacity: 2 });
  const expiresAt = new Date(Date.now() + 60_000).toISOString();
  let revoked = false, ca = await readFile(join(directory, "ca.crt"), "utf8");
  const control = async (body: Record<string, string>) => {
    if (revoked) throw Error("revoked");
    if (body.action === "exchange") return { token: randomUUID(), scopeID: "123", expiresAt };
    return { kind: "managed", scopeID: "123", subject: "fixture", connectionID: "123", profile: "schema", revision: "one", expiresAt,
      database: { host: "127.0.0.1", port, database: "tenant", username: "tenant", password: "managed-fixture-password", caCertificate: ca } };
  };
  let origin: string;
  const gatewayContainer = container + "-gateway";
  if (process.env.WEBDOCK_MANAGED_RUNTIME_TEST_GATEWAY_BINARY) {
    // The existing real database/TLS acceptance now also runs against the native Rust gateway.
    const api = createServer(async (req, res) => {
      try {
        assert.equal(req.headers.authorization, "Bearer " + "fixture-secret-".repeat(3));
        let body = ""; for await (const chunk of req) body += chunk;
        res.setHeader("Content-Type", "application/json"); res.end(JSON.stringify({ data: await control(JSON.parse(body)) }));
      } catch { res.writeHead(401); res.end(); }
    });
    api.listen(0, "127.0.0.1"); await once(api, "listening");
    t.after(() => { api.closeAllConnections(); api.close(); });
    const address = api.address(); assert.ok(address && typeof address !== "string");
    const reserve=createServer(); reserve.listen(0,"127.0.0.1"); await once(reserve,"listening");
    const reserved=reserve.address(); assert.ok(reserved && typeof reserved!=="string");
    await new Promise<void>(resolve=>reserve.close(()=>resolve()));
    origin=`http://127.0.0.1:${reserved.port}`;
    const pin=JSON.parse(await readFile(new URL("../../../vendor/tabularis-web-ui.json",import.meta.url),"utf8"));
    const child=spawn(process.env.WEBDOCK_MANAGED_RUNTIME_TEST_GATEWAY_BINARY,[],{stdio:"ignore",env:{
      PATH:process.env.PATH,LANG:"C.UTF-8",RUST_LOG:"off",HOST:"127.0.0.1",PORT:String(reserved.port),
      WEBDOCK_AUTH_ORIGIN:`http://127.0.0.1:${address.port}`,WEBDOCK_DATABASE_GATEWAY_ORIGIN:origin,
      WEBDOCK_DATABASE_BROWSER_ORIGINS:"http://localhost:3120",WEBDOCK_DATABASE_GATEWAY_SECRET:"fixture-secret-".repeat(3),
      WEBDOCK_DATABASE_MANAGED_RUNTIMES:"true",WEBDOCK_DATABASE_RUNTIME_CAPACITY:"2",WEBDOCK_DATABASE_RUNTIME_DIRECTORY:directory,
      TABULARIS_EXPECTED_COMMIT:pin.sourceCommit,TABULARIS_EXECUTABLE:process.env.WEBDOCK_MANAGED_RUNTIME_TEST_BINARY,
      TABULARIS_WEB_ROOT:process.env.WEBDOCK_MANAGED_RUNTIME_TEST_WEB_ROOT,
    }});
    const exited=once(child,"exit");t.after(async()=>{if(child.exitCode===null&&child.signalCode===null){child.kill("SIGTERM");const force=setTimeout(()=>child.kill("SIGKILL"),2000);try{await exited;}finally{clearTimeout(force);}}});
    let available=false;for(let i=0;i<100&&!available;i++){try{available=(await fetch(origin+"/healthz")).ok;}catch{await new Promise(resolve=>setTimeout(resolve,50));}}
    assert.ok(available,"Native database gateway must start");
  } else   if (process.env.WEBDOCK_MANAGED_RUNTIME_TEST_IMAGE && !process.env.WEBDOCK_MANAGED_RUNTIME_TEST_GATEWAY_BINARY) {
    const api = createServer(async (req, res) => {
      try {
        assert.equal(req.headers.authorization, "Bearer " + "fixture-secret-".repeat(3));
        let body = ""; for await (const chunk of req) body += chunk;
        res.setHeader("Content-Type", "application/json"); res.end(JSON.stringify({ data: await control(JSON.parse(body)) }));
      } catch { res.writeHead(401); res.end(); }
    });
    api.listen(0, "127.0.0.1"); await once(api, "listening");
    t.after(() => { api.closeAllConnections(); api.close(); });
    const address = api.address(); assert.ok(address && typeof address !== "string");
    const reserve = createServer(); reserve.listen(0, "127.0.0.1"); await once(reserve, "listening");
    const reserved = reserve.address(); assert.ok(reserved && typeof reserved !== "string");
    await new Promise<void>(resolve => reserve.close(() => resolve()));
    origin = `http://127.0.0.1:${reserved.port}`;
    const pin = JSON.parse(await readFile(new URL("../../../vendor/tabularis-web-ui.json", import.meta.url), "utf8"));
    t.after(() => { try { execFileSync("docker", ["rm", "-f", gatewayContainer], { stdio: "ignore" }); } catch {} });
    execFileSync("docker", ["run", "--detach", "--rm", "--name", gatewayContainer, "--network", "host", "--read-only", "--cap-drop=ALL", "--security-opt=no-new-privileges", "--pids-limit=256", "--memory=512m", "--cpus=1", "--tmpfs", "/tmp:rw,nosuid,nodev,size=128m,mode=1777",
      "-e", `HOST=127.0.0.1`, "-e", `PORT=${reserved.port}`, "-e", `WEBDOCK_AUTH_ORIGIN=http://127.0.0.1:${address.port}`,
      "-e", `WEBDOCK_DATABASE_GATEWAY_ORIGIN=${origin}`, "-e", "WEBDOCK_DATABASE_BROWSER_ORIGINS=http://localhost:3120",
      "-e", "WEBDOCK_DATABASE_GATEWAY_SECRET=" + "fixture-secret-".repeat(3), "-e", "WEBDOCK_DATABASE_MANAGED_RUNTIMES=true",
      "-e", "WEBDOCK_DATABASE_RUNTIME_CAPACITY=2", "-e", `TABULARIS_EXPECTED_COMMIT=${pin.sourceCommit}`, process.env.WEBDOCK_MANAGED_RUNTIME_TEST_IMAGE], { stdio: "ignore" });
    assert.equal(execFileSync("docker", ["exec", gatewayContainer, "id", "-u"], { encoding: "utf8" }).trim(), "65532");
    let available = false;
    for (let i = 0; i < 50 && !available; i++) {
      try { available = (await fetch(origin + "/healthz")).ok; } catch { await new Promise(resolve => setTimeout(resolve, 100)); }
    }
    assert.ok(available);
  } else {
    const gateway = createGateway({ publicOrigin: "http://localhost:3137", allowedOrigins: ["http://localhost:3120"], runtimeOrigins: [], managedRuntimes: manager, control });
    t.after(() => gateway.shutdown());
    gateway.listen(0, "127.0.0.1"); await once(gateway, "listening");
    const address = gateway.address(); assert.ok(address && typeof address !== "string");
    origin = `http://127.0.0.1:${address.port}`;
  }
  const common = { Origin: "http://localhost:3120", "Content-Type": "application/json" };
  const launch = await fetch(origin + "/session", { method: "POST", headers: common, body: JSON.stringify({ code: "x".repeat(43) }) });
  assert.equal(launch.status, 200, await launch.text());
  const headers = { ...common, Cookie: launch.headers.get("set-cookie")!.split(";")[0] };
  const session = await fetch(origin + "/s/123/api/v1/session", { headers });
  const negotiation = await session.json(); assert.equal(session.status, 200, JSON.stringify(negotiation));
  assert.equal(negotiation.access.highRiskCapabilities, false);
  const rpc = async (command: string, args: unknown) => {
    const response = await fetch(origin + "/s/123/api/v1/rpc/" + command, { method: "POST", headers: { ...headers, "X-Tabularis-Csrf": negotiation.csrfToken }, body: JSON.stringify(args) });
    return { status: response.status, body: await response.json() };
  };
  const connections = await rpc("get_connections", null);
  assert.equal(connections.body.data.length, 1); assert.ok(!JSON.stringify(connections.body).includes("managed-fixture-password"));
  const query = (query: string) => rpc("execute_query", { connectionId: "123", query, limit: 10 });
  for (const sql of ["CREATE TABLE viewer_fixture(id integer primary key, value text)", "INSERT INTO viewer_fixture VALUES(1, 'first')", "UPDATE viewer_fixture SET value='edited' WHERE id=1", "ALTER TABLE viewer_fixture ADD COLUMN enabled boolean DEFAULT true"]) {
    const result = await query(sql); assert.equal(result.body.ok, true, JSON.stringify(result));
  }
  const rows = await query("SELECT value,enabled FROM viewer_fixture");
  assert.equal(rows.body.ok, true, JSON.stringify(rows)); assert.deepEqual(rows.body.data.rows, [["edited", true]]);
  const tls = await query("SELECT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid()"); assert.deepEqual(tls.body.data.rows, [[true]]);
  const crossTarget = await rpc("execute_query", { connectionId: "456", query: "select 1" }); assert.equal(crossTarget.status, 403);
  revoked = true; assert.equal((await query("SELECT 1")).status, 401);
  if (process.env.WEBDOCK_MANAGED_RUNTIME_TEST_IMAGE && !process.env.WEBDOCK_MANAGED_RUNTIME_TEST_GATEWAY_BINARY) {
    const files = execFileSync("docker", ["exec", gatewayContainer, "find", "/tmp", "-maxdepth", "1", "-type", "d", "(", "-name", "webdock-tabularis-*", "-o", "-name", "webdock-database-*", ")"], { encoding: "utf8" });
    assert.equal(files.trim(), "");
  } else assert.deepEqual((await readdir(directory)).filter(name => name.startsWith("webdock-tabularis-") || name.startsWith("webdock-database-")), []);

  // A new process with an unrelated CA must not connect, proving verification is enforced.
  revoked = false;
  execFileSync("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", join(directory, "wrong.key"), "-out", join(directory, "wrong.crt"), "-subj", "/CN=wrong", "-days", "1"], { stdio: "ignore" });
  ca = await readFile(join(directory, "wrong.crt"), "utf8");
  const wrongLaunch = await fetch(origin + "/session", { method: "POST", headers: common, body: JSON.stringify({ code: "y".repeat(43) }) }); assert.equal(wrongLaunch.status, 200);
  headers.Cookie = wrongLaunch.headers.get("set-cookie")!.split(";")[0];
  const wrongSession = await fetch(origin + "/s/123/api/v1/session", { headers }).then(r => r.json()); negotiation.csrfToken = wrongSession.csrfToken;
  assert.equal((await query("SELECT 1")).body.ok, false);
});
