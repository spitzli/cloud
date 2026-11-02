import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { randomBytes, randomUUID, generateKeyPairSync, sign } from "node:crypto";
import { request as httpsRequest } from "node:https";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { setTimeout as sleep } from "node:timers/promises";

const image = process.env.PLUGIN_TEST_IMAGE ?? "webdock-managed-cms:0.1.0-alpha.1";
const repo = fileURLToPath(new URL("../../../", import.meta.url));
const dir = await mkdtemp(join(tmpdir(), "cms-image-"));
const name = "webdock-cms-image-" + randomBytes(8).toString("hex");
const label = "webdock.cms-image";
const runDocker = (args, environment = {}) => execFileSync("docker", args, {
  encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 60000,
  env: { ...process.env, ...environment },
}).trim();
const docker = (...args) => runDocker(args);
try {
  const keys = generateKeyPairSync("ed25519");
  const publicKey = keys.publicKey.export({ type: "spki", format: "pem" }).toString();
  const token = randomBytes(32).toString("hex");
  const localDigest = docker("image", "inspect", image, "--format", "{{.Id}}");
  const manifest = JSON.parse(execFileSync(process.execPath, ["-e",
    'import { createManifest } from "./extensions/managed-cms/src/manifest.ts"; process.stdout.write(JSON.stringify(createManifest(process.argv[1])));',
    `ghcr.io/spitzli/webdock/managed-cms@${localDigest}`], { cwd: repo, encoding: "utf8" }));
  // Use the real installer renderer and TLS generator; never construct an alternate launch contract.
  const resources = JSON.parse(execFileSync("python3", ["-c", `
import sys,json,pathlib
sys.path.insert(0,'apps/build-worker')
import plugin_installer
value=json.load(sys.stdin)
config=json.loads(pathlib.Path('apps/build-worker/plugin-installer.example.json').read_text())
config['runtime'].update(hostOrigin='https://callback.example.test',consoleOrigin='https://console.example.test',apiAddresses=['203.0.113.10'])
certificate,key=plugin_installer.generate_runtime_tls('localhost')
job={'id':'123','manifestRaw':json.dumps(value['manifest']),'configuration':{}}
print(json.dumps(plugin_installer.runtime_resources(config,job,value['token'],value['publicKey'],certificate,key)))
`], { cwd: repo, input: JSON.stringify({ manifest, token, publicKey }), encoding: "utf8", stdio: ["pipe", "pipe", "pipe"] }));
  const identity = resources.find(item => item.kind === "Secret" && item.type === "Opaque").stringData;
  const tls = resources.find(item => item.kind === "Secret" && item.type === "kubernetes.io/tls").stringData;
  const pod = resources.find(item => item.kind === "Deployment").spec.template.spec;
  assert.equal(identity.WEBDOCK_PLUGIN_PUBLIC_KEY, publicKey);
  assert.ok(identity.WEBDOCK_PLUGIN_RUNTIME_TOKEN === token, "Installer retains the runtime credential");
  assert.equal(identity.WEBDOCK_PLUGIN_CONSOLE_ORIGIN, "https://console.example.test");
  assert.ok(!Object.hasOwn(identity, "WEBDOCK_PLUGIN_HOST_PUBLIC_KEY"));
  assert.equal(pod.securityContext.runAsUser, 1000);
  assert.equal(pod.volumes.find(volume => volume.name === "tls").secret.defaultMode, 0o440);
  // Host fixture remains 0700; individual readonly binds expose only the generated TLS files.
  for (const [file, value] of Object.entries(tls)) await writeFile(join(dir, file), value, { mode: 0o444 });
  const environment = Object.keys(identity).flatMap(key => ["--env", key]);
  runDocker(["run", "-d", "--name", name, "--label", `${label}=${name}`, "--user", `${pod.securityContext.runAsUser}:${pod.securityContext.runAsGroup}`,
    "--read-only", "--cap-drop=ALL", "--security-opt=no-new-privileges", "--network", "bridge", ...environment,
    ...Object.keys(tls).flatMap(file => ["--mount", `type=bind,src=${join(dir, file)},dst=/runtime-tls/${file},readonly`]),
    "--tmpfs", "/tmp:rw,nosuid,nodev,size=32m,mode=1777", "-p", "127.0.0.1::3443", "-p", "127.0.0.1::3444", image], identity);
  const tlsPort = Number(docker("port", name, "3443/tcp").split(":").at(-1));
  const ui = "http://" + docker("port", name, "3444/tcp");
  let ready = false;
  for (let n = 0; n < 60; n++) {
    try { if ((await fetch(ui + "/ui/cms")).status === 200) { ready = true; break; } } catch {}
    await sleep(200);
  }
  assert.ok(ready, "Image starts with installer-generated environment and UID1000");
  const html = await fetch(ui + "/ui/cms");
  assert.match(html.headers.get("content-security-policy"), /frame-ancestors https:\/\/console.example.test;/);
  assert.match(await html.text(), /<html/i);
  for (const asset of ["cms.js", "cms.css"]) {
    const response = await fetch(ui + "/ui/" + asset);
    assert.equal(response.status, 200);
    assert.ok((await response.text()).length > 0);
  }
  for (const action of ["cms.instances", "cms.create", "cms.read", "cms.write"]) {
    assert.equal((await fetch(ui + "/actions/" + action, { method: "POST" })).status, 404);
  }
  const https = (path, body, headers = {}) => new Promise((resolve, reject) => {
    const request = httpsRequest({ hostname: "localhost", family: 4, port: tlsPort, path, ca: tls["tls.crt"],
      method: body === undefined ? "GET" : "POST", headers: { "content-type": "application/json", ...headers } }, response => {
      const chunks = [];
      response.on("data", part => chunks.push(part));
      response.on("end", () => resolve({ status: response.statusCode, body: JSON.parse(Buffer.concat(chunks).toString()) }));
    });
    request.on("error", reject);request.setTimeout(10000, () => request.destroy(new Error("Fixture HTTPS timeout")));
    request.end(body === undefined ? undefined : JSON.stringify(body));
  });
  const health = await https("/healthz");
  assert.equal(health.status, 200);
  assert.equal(health.body.pluginID, identity.WEBDOCK_PLUGIN_ID);
  assert.equal(health.body.version, identity.WEBDOCK_PLUGIN_VERSION);
  assert.equal(health.body.generation, Number(identity.WEBDOCK_PLUGIN_GENERATION));
  const payload = { input: { invalid: true }, configuration: {}, hostOrigin: identity.WEBDOCK_PLUGIN_HOST_URL };
  assert.equal((await https("/actions/cms.instances", payload)).status, 401);
  const invocation = privateKey => {
    const now = Math.floor(Date.now() / 1000);
    const body = Buffer.from(JSON.stringify({ v: 1, id: randomUUID(), pluginID: identity.WEBDOCK_PLUGIN_ID, generation: 1,
      action: "cms.instances", projectID: "123", customerID: "101", iat: now, exp: now + 50 })).toString("base64url");
    return body + "." + sign(null, Buffer.from(body), privateKey).toString("base64url");
  };
  const invoke = privateKey => https("/actions/cms.instances", payload, { authorization: "Bearer " + token, "x-plugin-invocation": invocation(privateKey) });
  assert.equal((await invoke(generateKeyPairSync("ed25519").privateKey)).status, 403);
  // An authentic invocation reaches input validation; no host callback or customer mutation occurs.
  assert.equal((await invoke(keys.privateKey)).status, 400);
  assert.equal(docker("inspect", "--format", "{{.Config.User}}", name), "1000:1000");
  assert.equal(docker("image", "inspect", image, "--format", "{{.Config.User}}"), "65532:65532");
  console.log(JSON.stringify({ passed: true, image, installerUser: "1000:1000", imageDefaultUser: "65532:65532",
    checks: ["actual installer Secret environment and TLS paths", "native image command with installer UID1000", "verified TLS health identity",
      "public UI assets and exact Console CSP", "public actions isolated", "runtime token and standard PUBLIC_KEY signature enforced"] }));
} finally {
  try {
    let owner;
    try { owner = docker("inspect", "--format", `{{index .Config.Labels "${label}"}}`, name); } catch {}
    if (owner !== undefined) {
      assert.equal(owner, name, "Only this smoke test's container may be removed");
      docker("rm", "-f", name);
    }
  } finally { await rm(dir, { recursive: true, force: true }); }
}
