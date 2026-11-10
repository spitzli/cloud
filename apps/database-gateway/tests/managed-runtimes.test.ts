import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ManagedRuntimeAccess } from "@spitzli/database-contracts";
import { createManagedRuntimeManager } from "../src/managed-runtimes";

const access = (changes: Partial<ManagedRuntimeAccess> = {}): ManagedRuntimeAccess => ({
  kind: "managed", scopeID: "123", subject: "alice", connectionID: "123", profile: "schema", revision: "revision-1",
  expiresAt: new Date(Date.now() + 60_000).toISOString(),
  database: { host: "database.internal", port: 5432, database: "tenant", username: "owner", password: "private-password", caCertificate: "private-ca" }, ...changes,
});
async function fixture() {
  const root = await mkdtemp(join(tmpdir(), "managed-test-"));
  const executable = join(root, "runtime");
  await writeFile(executable, `#!${process.execPath}
const {createServer} = require('node:http');
const {writeFileSync} = require('node:fs');
const args = process.argv;
writeFileSync(process.env.TABULARIS_DATA_DIR+'/process.json', JSON.stringify({pid:process.pid,env:process.env,args}));
createServer((req,res)=>{res.setHeader('Content-Type','application/json');res.end(JSON.stringify({ok:true}));}).listen(Number(args[args.indexOf('--port')+1]),'127.0.0.1');
`, { mode: 0o700 });
  return { root, executable, cleanup: () => rm(root, { recursive: true, force: true }) };
}

test("managed runtimes isolate sessions, save one verified connection, and single-flight initialization", async () => {
  const f = await fixture(); const manager = createManagedRuntimeManager({ executable: f.executable, temporaryDirectory: f.root, capacity: 2 });
  try {
    const [first, again] = await Promise.all([manager.acquire("one", access()), manager.acquire("one", access())]);
    assert.deepEqual(first, again);
    const second = await manager.acquire("two", access());
    assert.notEqual(first.runtimeOrigin, second.runtimeOrigin); assert.notEqual(first.proxySecret, second.proxySecret);
    assert.match(first.runtimeOrigin, /^http:\/\/127\.0\.0\.1:\d+$/); assert.equal(first.authenticationOrigin, first.runtimeOrigin.replace("http:", "https:"));
    assert.ok(!JSON.stringify(first).includes("private-password"));
    const directories = (await readdir(f.root)).filter(name => name.startsWith("webdock-tabularis-")); assert.equal(directories.length, 2);
    for (const name of directories) {
      const directory = join(f.root, name); assert.equal((await stat(directory)).mode & 0o777, 0o700);
      const connections = JSON.parse(await readFile(join(directory, "connections.json"), "utf8"));
      assert.equal(connections.length, 1); assert.equal(connections[0].id, "123");
      assert.deepEqual(connections[0].params, { driver: "postgres", host: "database.internal", port: 5432, database: "tenant", username: "owner", password: "private-password", ssl_mode: "verify-full", ssl_ca: join(directory, "ca.crt"), save_in_keychain: false });
      assert.equal((await stat(join(directory, "connections.json"))).mode & 0o777, 0o600);
      assert.equal(await readFile(join(directory, "ca.crt"), "utf8"), "private-ca");
      const processInfo = JSON.parse(await readFile(join(directory, "process.json"), "utf8"));
      assert.equal(processInfo.env.SPITZLI_DATABASE_GATEWAY_SECRET, undefined); assert.equal(processInfo.env.RUST_LOG, "off");
      assert.ok(!JSON.stringify(processInfo.args).includes("private-password"));
    }
    await assert.rejects(manager.acquire("three", access()), /capacity/i);
    await assert.rejects(manager.acquire("one", access({ revision: "revision-2" })), /changed/i);
    await manager.release("one");
    assert.equal((await readdir(f.root)).filter(name => name.startsWith("webdock-tabularis-")).length, 1);
    await manager.shutdown();
    assert.deepEqual(await readdir(f.root), ["runtime"]);
    await assert.rejects(manager.acquire("new", access()), /unavailable/i);
  } finally { await manager.shutdown(); await f.cleanup(); }
});

test("failed spawn releases capacity and removes private files without reporting secrets", async () => {
  const f = await fixture(); const manager = createManagedRuntimeManager({ executable: join(f.root, "absent"), temporaryDirectory: f.root, capacity: 1, startupTimeoutMs: 100 });
  try {
    for (let i = 0; i < 2; i++) {
      await assert.rejects(manager.acquire(String(i), access()), error => error instanceof Error && /start/i.test(error.message) && !error.message.includes(f.root) && !error.message.includes("private"));
      assert.deepEqual(await readdir(f.root), ["runtime"]);
    }
  } finally { await manager.shutdown(); await f.cleanup(); }
});

test("release during initialization and startup timeout terminate children and erase state", async () => {
  const f = await fixture();
  await writeFile(f.executable, `#!${process.execPath}\nsetInterval(()=>{},1000);\n`, { mode: 0o700 });
  const manager = createManagedRuntimeManager({ executable: f.executable, temporaryDirectory: f.root, capacity: 1, startupTimeoutMs: 100 });
  try {
    const starting = manager.acquire("one", access()); const rejected = assert.rejects(starting, /start|ended/i);
    await manager.release("one"); await rejected;
    assert.deepEqual(await readdir(f.root), ["runtime"]);
    await assert.rejects(manager.acquire("two", access()), /start/i);
    assert.deepEqual(await readdir(f.root), ["runtime"]);
  } finally { await manager.shutdown(); await f.cleanup(); }
});


test("managed process expires without another request and removes its child and files", async () => {
  const f = await fixture(); const manager = createManagedRuntimeManager({ executable: f.executable, temporaryDirectory: f.root, capacity: 1 });
  try {
    await manager.acquire("short", access({ expiresAt: new Date(Date.now() + 250).toISOString() }));
    const directory = (await readdir(f.root)).find(name => name.startsWith("webdock-tabularis-"))!;
    const { pid } = JSON.parse(await readFile(join(f.root, directory, "process.json"), "utf8"));
    await new Promise(resolve => setTimeout(resolve, 300));
    assert.throws(() => process.kill(pid, 0), { code: "ESRCH" });
    assert.deepEqual(await readdir(f.root), ["runtime"]);
  } finally { await manager.shutdown(); await f.cleanup(); }
});
