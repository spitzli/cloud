// Local development helper used by the root Tiltfile. Owned loopback state only; never deployment config.
// Usage: bun tools/dev/local.mjs secrets|init|plugin-image <id>|seed|kubeconfig|cluster-ca
import { execFileSync } from "node:child_process";
import { createHash, generateKeyPairSync, randomBytes, scryptSync } from "node:crypto";
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const root = resolve(import.meta.dirname, "../..");
const state = join(root, ".dev-local");
const compose = ["compose", "-f", join(root, "deploy/dev/compose.yaml")];
const port = Number(process.env.WEBDOCK_DEV_POSTGRES_PORT ?? 55440);
const target = `127.0.0.1:${port}/webdock`;
// WEBDOCK_DEV_HTTPS=1 serves Console and account pages over HTTPS (some password managers refuse
// passkeys on plain-HTTP localhost). Origins are recorded at bootstrap, so switching needs a reset.
const https = process.env.WEBDOCK_DEV_HTTPS === "1";
const scheme = https ? "https" : "http";
const origins = {
  WEBDOCK_API_ORIGIN: "http://127.0.0.1:3130",
  WEBDOCK_AUTH_ORIGIN: `${scheme}://localhost:3125`,
  WEBDOCK_STUDIO_ORIGIN: `${scheme}://127.0.0.1:3120`,
  WEBDOCK_CONSOLE_ORIGIN: `${scheme}://127.0.0.1:3121`,
};
const roles = [
  "webdock_auth_bootstrap", "webdock_auth_runtime", "webdock_worker_runtime", "webdock_admin_runtime",
  "webdock_registry_runtime", "webdock_runtime", "webdock_browser_website", "webdock_cms_archive_runtime",
  "webdock_offline_migrate",
];
const secret = () => randomBytes(32).toString("hex");
const file = (name) => join(state, name);
function privateJSON(name, value) {
  writeFileSync(file(name), JSON.stringify(value, null, 2), { mode: 0o600 });
}
const url = (role, password) => `postgresql://${role}:${password}@${target}`;

function tls() {
  const dir = join(state, "tls");
  if (!https || existsSync(join(dir, "cert.pem"))) return;
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  try {
    execFileSync("mkcert", ["-cert-file", join(dir, "cert.pem"), "-key-file", join(dir, "key.pem"), "localhost", "127.0.0.1"], { stdio: "inherit" });
  } catch {
    throw new Error("WEBDOCK_DEV_HTTPS=1 needs mkcert with its local CA installed: install mkcert, run `mkcert -install` once, then restart Tilt.");
  }
}
function secrets() {
  mkdirSync(state, { recursive: true, mode: 0o700 });
  tls();
  // The shared customer database server for the postgres plugin; generated once, like the rest.
  if (!existsSync(file("shared-postgres.env"))) writeFileSync(file("shared-postgres.env"), `POSTGRES_PASSWORD=${secret()}\n`, { mode: 0o600 });
  if (existsSync(file("secrets.json"))) return JSON.parse(readFileSync(file("secrets.json"), "utf8"));
  const value = {
    postgres: secret(), rls: secret(), auth: secret(), operatorPassword: `Dev-${secret().slice(0, 20)}!`,
    roles: Object.fromEntries(roles.map((role) => [role, secret()])),
  };
  privateJSON("secrets.json", value);
  privateJSON("roles.json", value.roles);
  writeFileSync(file("postgres.env"), `POSTGRES_PASSWORD=${value.postgres}\n`, { mode: 0o600 });
  return value;
}

function waitForDatabase() {
  // Compose reports the container as running while initdb is still creating the database.
  for (let attempt = 0; attempt < 120; attempt++) {
    try {
      execFileSync("docker", [...compose, "exec", "-T", "postgres", "pg_isready", "-h", "127.0.0.1", "-U", "postgres", "-d", "webdock"], { stdio: "ignore" });
      return;
    } catch { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 500); }
  }
  throw new Error("Local PostgreSQL did not become ready");
}
function initialized() {
  const sql = "SELECT to_regclass('webdock_auth.user') IS NOT NULL";
  return execFileSync("docker", [...compose, "exec", "-T", "postgres", "psql", "-h", "127.0.0.1", "-U", "postgres", "-d", "webdock", "-tAc", sql], { encoding: "utf8" }).trim() === "t";
}

function run(binary, args, env) {
  execFileSync(join(root, "target/debug", binary), args, {
    stdio: "inherit", cwd: root,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, NODE_ENV: "development", ...env },
  });
}

function init(value) {
  const admin = { WEBDOCK_MIGRATION_DATABASE_URL: url("postgres", value.postgres), DATABASE_RLS_SECRET: value.rls };
  waitForDatabase();
  if (!initialized()) {
    // A fresh volume invalidates any operator output from a previous database.
    rmSync(file("initial-secrets.json"), { force: true });
    run("webdock-migrate", ["--target", target, "--initialize-empty", "--provision-roles-from", file("roles.json")], admin);
  } else {
    run("webdock-migrate", ["--target", target], admin);
  }
  if (existsSync(file("initial-secrets.json"))) {
    // Sign-in redirect URIs were registered with the scheme used at bootstrap.
    const recorded = existsSync(file("scheme")) ? readFileSync(file("scheme"), "utf8").trim() : "http";
    if (recorded !== scheme) throw new Error(`This database was set up for ${recorded}. Reset it (docker compose -f deploy/dev/compose.yaml -p webdock-dev down -v && rm -rf .dev-local) to switch to ${scheme}.`);
    return;
  }
  privateJSON("operator.json", {
    email: "operator@example.test", name: "Local operator", password: value.operatorPassword,
    authOrigin: origins.WEBDOCK_AUTH_ORIGIN, studioOrigin: origins.WEBDOCK_STUDIO_ORIGIN, consoleOrigin: origins.WEBDOCK_CONSOLE_ORIGIN,
  });
  run("webdock-bootstrap-operator", ["--target", target, "--config", file("operator.json"), "--output", file("initial-secrets.json")], {
    ...origins, WEBDOCK_AUTH_SECRET: value.auth, DATABASE_URL_UNPOOLED: url("webdock_offline_migrate", value.roles.webdock_offline_migrate),
  });
  writeFileSync(file("scheme"), scheme, { mode: 0o600 });
  console.log(`Local operator: operator@example.test / ${value.operatorPassword} (password change and MFA are required at first sign-in)`);
}

// Local plugins: what a reviewed installer would record, plus private co-located Unix sockets.
// Each runs natively from its own workspace (see the Tiltfile); the image in the local registry is
// the release reference the Core pins.
const plugins = join(state, "plugins");
const registry = `127.0.0.1:${process.env.WEBDOCK_DEV_REGISTRY_PORT ?? 5005}`;
const sharedPort = Number(process.env.WEBDOCK_DEV_SHARED_POSTGRES_PORT ?? 55441);
const demo = { customer: "900000000000000101", organization: "900000000000000102", project: "900000000000000103" };
const pluginFile = (name) => join(plugins, name);
const uid = process.getuid();
// storage: the plugin gets its own database and login role in the local server.
// policy: local-dev self-service defaults, so the Demo customer can try every feature.
const PLUGINS = {
  "echo-demo": { policy: { "create-notes": "selected" } },
  postgres: {
    storage: true,
    capabilities: ["plugin.workloads"],
    routeDomains: ["db.localhost"],
    policy: { "create-shared": "selected", "create-dedicated": "selected" },
    config: (secrets) => ({
      secretKeyHex: secrets.sealKey,
      sharedServers: [{
        id: "local-1", label: "Local shared server", host: "127.0.0.1", port: sharedPort,
        adminUrl: `postgresql://postgres:${readEnv("shared-postgres.env").POSTGRES_PASSWORD}@127.0.0.1:${sharedPort}/postgres`, capacity: 50,
      }],
      // CloudNativePG on the local k3s, reached through Traefik's PostgreSQL entry point by TLS server name.
      dedicated: { domain: "db.localhost", port: Number(process.env.WEBDOCK_DEV_K3S_POSTGRES_PORT ?? 55442), entryPoints: ["postgres"], issuer: "webdock-dev-ca" },
    }),
  },
  // Every message goes to the local Mailpit catcher; nothing leaves this machine.
  mail: {
    storage: true,
    capabilities: ["plugin.workloads"],
    routeDomains: ["mail.localhost", "mail-admin.localhost"],
    policy: { "add-domains": "selected", "byok-turbosmtp": "selected", "create-mailboxes": "selected" },
    config: (secrets) => ({
      secretKeyHex: secrets.sealKey,
      platformTransport: { kind: "smtp", host: "127.0.0.1", port: Number(process.env.WEBDOCK_DEV_SMTP_PORT ?? 55025) },
      trustUnverifiedDomains: true,
      // One Stalwart CE server per project on the local k3s, egress denied; IMAP and submission through
      // Traefik by TLS server name, management over its own route on the HTTP entry point.
      mailboxes: {
        image: "docker.io/stalwartlabs/stalwart@sha256:74e5a7d55303ba525d939c6bf97ed4e010df7521f52d80afc22a815b66bd53f3",
        domain: "mail.localhost",
        imapPort: Number(process.env.WEBDOCK_DEV_K3S_IMAPS_PORT ?? 55993),
        submissionPort: Number(process.env.WEBDOCK_DEV_K3S_SUBMISSIONS_PORT ?? 55465),
        admin: { origin: `http://127.0.0.1:${process.env.WEBDOCK_DEV_K3S_HTTP_PORT ?? 55480}`, domain: "mail-admin.localhost", entryPoints: ["web"] },
        issuer: "webdock-dev-ca",
      },
    }),
  },
  hosting: {
    storage: true,
    capabilities: ["plugin.workloads"],
    routeDomains: ["apps.localhost"],
    policy: { "create-apps": "selected", "resize-apps": "selected" },
    config: (secrets) => ({
      secretKeyHex: secrets.sealKey,
      // Apps run on the local k3s behind its Traefik, over HTTPS with certificates from the local CA.
      placement: { domain: "apps.localhost", publicUrl: `https://{host}:${process.env.WEBDOCK_DEV_K3S_HTTPS_PORT ?? 55481}`, entryPoints: ["websecure"], issuer: "webdock-dev-ca" },
    }),
  },
};
function readEnv(name) {
  return Object.fromEntries(readFileSync(file(name), "utf8").trim().split("\n").map((line) => line.split(/=(.*)/s).slice(0, 2)));
}
function psql(sql, variables = {}, database = "webdock") {
  const args = [...compose, "exec", "-T", "postgres", "psql", "-h", "127.0.0.1", "-U", "postgres", "-d", database, "-v", "ON_ERROR_STOP=1", "-tA"];
  for (const [name, value] of Object.entries(variables)) args.push("-v", `${name}=${value}`);
  return execFileSync("docker", args, { input: sql, encoding: "utf8" }).trim();
}
function pluginSecrets(id) {
  const path = pluginFile(`${id}.secrets.json`);
  if (existsSync(path)) return JSON.parse(readFileSync(path, "utf8"));
  const value = { storage: secret(), sealKey: secret() };
  writeFileSync(path, JSON.stringify(value), { mode: 0o600 });
  return value;
}

async function pluginImage(id) {
  mkdirSync(plugins, { recursive: true, mode: 0o700 });
  if (existsSync(pluginFile(`${id}.image`))) {
    // A reset registry volume no longer holds the recorded digest; rebuild instead of seeding a dangling reference.
    const digest = readFileSync(pluginFile(`${id}.image`), "utf8").trim().split("@")[1];
    const head = await fetch(`http://${registry}/v2/webdock/${id}/manifests/${digest}`, {
      method: "HEAD", headers: { Accept: "application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json" },
    }).catch(() => null);
    if (head?.ok) return;
  }
  const tag = `${registry}/webdock/${id}:dev`;
  execFileSync("docker", ["build", "-f", `plugins/${id}/Dockerfile`, "-t", tag, "."], { cwd: root, stdio: "inherit" });
  execFileSync("docker", ["push", tag], { stdio: "inherit" });
  const digests = JSON.parse(execFileSync("docker", ["image", "inspect", "--format", "{{json .RepoDigests}}", tag], { encoding: "utf8" }));
  const pinned = digests.find((value) => value.startsWith(`${registry}/webdock/${id}@sha256:`));
  if (!pinned) throw new Error("Pushed image digest not found");
  writeFileSync(pluginFile(`${id}.image`), pinned, { mode: 0o600 });
}

function storage(id, password) {
  const name = `webdock_plugin_${id.replaceAll("-", "_")}`;
  // The role owns only its own database. It can still open the platform database (PUBLIC keeps
  // CONNECT there in local dev) but holds no grants on its schemas or tables.
  psql(`SELECT format('CREATE ROLE %I LOGIN PASSWORD %L', :'name', :'password') WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname=:'name') \\gexec
SELECT format('ALTER ROLE %I PASSWORD %L', :'name', :'password') \\gexec
SELECT format('CREATE DATABASE %I OWNER %I', :'name', :'name') WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname=:'name') \\gexec
SELECT format('REVOKE ALL ON DATABASE %I FROM PUBLIC', :'name') \\gexec`, { name, password });
  return `postgresql://${name}:${password}@127.0.0.1:${port}/${name}`;
}

function seedPlugin(id, credential) {
  const options = PLUGINS[id];
  // The stored release keeps its image across rebuilds. A changed repository manifest replaces the
  // local dev release in place: shadow mode has no upgrade path to preserve.
  const stored = psql("SELECT manifest_bytes FROM webdock_auth.plugin_release WHERE plugin_id=:'id'", { id });
  const value = JSON.parse(readFileSync(join(root, `plugins/${id}/manifest.json`), "utf8"));
  value.image = stored ? JSON.parse(stored).image : readFileSync(pluginFile(`${id}.image`), "utf8").trim();
  const manifest = JSON.stringify(value);
  writeFileSync(pluginFile(`${id}.manifest.json`), manifest, { mode: 0o644 });
  const policies = Object.entries(options.policy ?? {}).map(([feature, mode]) => `
INSERT INTO webdock_auth.plugin_feature_policy(plugin_id,feature_id,mode,updated_by) VALUES(:'id','${feature}','${mode}','local-dev') ON CONFLICT DO NOTHING;
INSERT INTO webdock_auth.plugin_feature_customer(plugin_id,feature_id,customer_id,added_by) VALUES(:'id','${feature}',:'customer','local-dev') ON CONFLICT DO NOTHING;`).join("");
  psql(`BEGIN;
INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES(:'id','external','enabled','enabled',:'version','ready',1) ON CONFLICT DO NOTHING;
INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES(:'id',:'version',:'manifest',encode(sha256(convert_to(:'manifest','UTF8')),'hex'),:'image',:'registry','local-dev') ON CONFLICT (plugin_id,version) DO UPDATE SET manifest_bytes=EXCLUDED.manifest_bytes,manifest_sha256=EXCLUDED.manifest_sha256 WHERE webdock_auth.plugin_release.manifest_bytes IS DISTINCT FROM EXCLUDED.manifest_bytes;
INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES(:'id',:'capabilities','{}') ON CONFLICT DO NOTHING;
UPDATE webdock_auth.plugin_configuration SET approved_capabilities=:'capabilities' WHERE plugin_id=:'id' AND approved_capabilities IS DISTINCT FROM (:'capabilities')::jsonb;
INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES(:'id',1,:'version',:'image','ready',0,:'credential','local',:'id') ON CONFLICT (plugin_id,generation) DO UPDATE SET credential_hash=EXCLUDED.credential_hash;
${policies}
UPDATE webdock_auth.plugin_runtime r SET configuration_revision=c.revision FROM webdock_auth.plugin_configuration c WHERE r.plugin_id=:'id' AND c.plugin_id=r.plugin_id AND r.configuration_revision<>c.revision;
COMMIT;`, { id, version: value.version, manifest, image: value.image, credential, registry: `http://${registry}`, customer: demo.customer, capabilities: JSON.stringify(options.capabilities ?? []) });
  const secrets = pluginSecrets(id);
  writeFileSync(pluginFile(`${id}.json`), JSON.stringify({
    manifest: pluginFile(`${id}.manifest.json`), issuerPublicKeyHex: readFileSync(pluginFile("invocation.pub"), "utf8"),
    generation: 1, listen: pluginFile(`${id}.sock`), core: pluginFile("control.sock"), corePeers: [{ kind: "unix", uid }],
    ...(options.storage ? { storageUrl: storage(id, secrets.storage) } : {}),
    ...(options.config ? options.config(secrets) : {}),
  }), { mode: 0o600 });
}

// A customer owner of the Demo customer, so self-service can be tried without the operator account.
// The password stays in .dev-local; it is never printed.
function demoOwner() {
  const path = file("demo-owner.json");
  if (!existsSync(path)) writeFileSync(path, JSON.stringify({ email: "owner@example.test", password: `Demo-${secret().slice(0, 20)}` }, null, 2), { mode: 0o600 });
  const { email, password } = JSON.parse(readFileSync(path, "utf8"));
  const salt = randomBytes(16).toString("hex");
  const hash = `${salt}:${scryptSync(password.normalize("NFKC"), salt, 64, { N: 16384, r: 16, p: 1, maxmem: 64 * 1024 * 1024 }).toString("hex")}`;
  const organization = psql("SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=:'customer'", { customer: demo.customer });
  psql(`BEGIN;
INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('local-demo-owner','Demo owner',:'email',true,'user',false,false) ON CONFLICT (id) DO NOTHING;
INSERT INTO webdock_auth.account(id,"accountId","providerId","userId",password,"createdAt","updatedAt") VALUES('local-demo-owner-account','local-demo-owner','credential','local-demo-owner',:'hash',now(),now()) ON CONFLICT (id) DO UPDATE SET password=EXCLUDED.password;
INSERT INTO webdock_auth.member(id,"organizationId","userId",role,"createdAt") VALUES('local-demo-owner-member',:'organization','local-demo-owner','owner',now()) ON CONFLICT (id) DO NOTHING;
COMMIT;`, { email, hash, organization });
}

// The local cluster's admin kubeconfig, readable by this user, pointing at the published API port.
function kubeconfig() {
  const dir = file("k3s");
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  let raw = "";
  for (let attempt = 0; attempt < 120 && !raw.includes("server:"); attempt++) {
    try { raw = execFileSync("docker", [...compose, "exec", "-T", "k3s", "cat", "/etc/rancher/k3s/k3s.yaml"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }); }
    catch { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 1000); }
  }
  if (!raw.includes("server:")) throw new Error("Local k3s did not write its kubeconfig");
  const api = process.env.WEBDOCK_DEV_K3S_API_PORT ?? 56443;
  writeFileSync(join(dir, "kubeconfig.yaml"), raw.replace("https://127.0.0.1:6443", `https://127.0.0.1:${api}`).replaceAll(": default", ": webdock-dev"), { mode: 0o600 });
}

// A CA of this machine's own for workload certificates, limited to *.localhost names, so trusting
// its root locally vouches for nothing else. cert-manager issues from it as ClusterIssuer webdock-dev-ca.
function clusterCa() {
  const dir = file("ca");
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const key = join(dir, "ca.key");
  const cert = join(dir, "ca.crt");
  if (!existsSync(cert)) {
    execFileSync("openssl", ["req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-keyout", key, "-out", cert, "-days", "3650",
      "-subj", "/CN=Webdock local development CA", "-addext", "basicConstraints=critical,CA:TRUE,pathlen:0",
      "-addext", "keyUsage=critical,keyCertSign,cRLSign", "-addext", "nameConstraints=critical,permitted;DNS:localhost,permitted;DNS:.localhost"], { stdio: "ignore" });
    chmodSync(key, 0o600);
  }
  const kubectl = (args, input) => execFileSync("kubectl", ["--kubeconfig", file("k3s/kubeconfig.yaml"), "--context", "webdock-dev", ...args], { input, encoding: "utf8", stdio: ["pipe", "pipe", "pipe"] });
  const secret = { apiVersion: "v1", kind: "Secret", type: "kubernetes.io/tls", metadata: { name: "webdock-dev-ca", namespace: "cert-manager" },
    data: { "tls.crt": readFileSync(cert).toString("base64"), "tls.key": readFileSync(key).toString("base64") } };
  const issuer = { apiVersion: "cert-manager.io/v1", kind: "ClusterIssuer", metadata: { name: "webdock-dev-ca" }, spec: { ca: { secretName: "webdock-dev-ca" } } };
  // cert-manager and its webhook come up a while after k3s.
  for (let attempt = 0; ; attempt++) {
    try {
      kubectl(["apply", "-f", "-"], JSON.stringify({ apiVersion: "v1", kind: "List", items: [secret, issuer] }));
      break;
    } catch {
      if (attempt >= 90) throw new Error("cert-manager did not accept the local CA");
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 2000);
    }
  }
  console.log(`Local CA ready: ${cert} (trust it to verify *.localhost workloads)`);
}

// The chart digest the agent checks before using a chart (same walk as ApprovedChart::digest).
function chartDigest(dir) {
  const files = [];
  const walk = (path) => {
    for (const name of readdirSync(path)) {
      const full = join(path, name);
      if (lstatSync(full).isDirectory()) walk(full);
      else files.push(relative(dir, full));
    }
  };
  walk(dir);
  files.sort();
  const hash = createHash("sha256");
  const length = (n) => { const b = Buffer.alloc(8); b.writeBigUInt64BE(BigInt(n)); return b; };
  for (const name of files) {
    const bytes = readFileSync(join(dir, name));
    hash.update(length(Buffer.byteLength(name))).update(name).update(length(bytes.length)).update(bytes);
  }
  return hash.digest("hex");
}

// One enrolled agent for the local k3s: it applies plugin workloads over a private Unix socket.
function agent() {
  const dir = file("agent");
  for (const path of [dir, join(dir, "state"), join(dir, "run")]) mkdirSync(path, { recursive: true, mode: 0o700 });
  const cluster = psql(`INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,generation,verified,capacity,runtime_kind,runtime_installation)
SELECT '900000000000000201','local-k3s','k3s','local','local development cluster',1,true,'{}','kubernetes','local'
WHERE NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_cluster WHERE name='local-k3s');
SELECT id FROM webdock_auth.hosting_cluster WHERE name='local-k3s';`).split("\n").pop();
  const credential = createHash("sha256").update(`local-dev-agent-${cluster}`).digest("hex");
  psql(`INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_sequence,observation)
SELECT :'cluster',:'credential',1,0,'{}' WHERE NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_agent WHERE cluster_id=:'cluster');
INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid)
SELECT 'local','local-agent',1,:'cluster',:'credential','unix',:'uid' WHERE NOT EXISTS(SELECT 1 FROM webdock_auth.agent_transport_enrollment WHERE installation='local' AND agent_id='local-agent');`, { cluster, credential, uid });
  const socket = join(dir, "run", "control.sock");
  writeFileSync(join(dir, "agent-control.json"), JSON.stringify({ kind: "unix", path: socket, installation: "local", agent_id: "local-agent", uid }), { mode: 0o600 });
  const chart = join(root, "deploy/helm/webdock-plugin");
  writeFileSync(join(dir, "agent.json"), JSON.stringify({
    version: 1, installation: "local", agentId: "local-agent", generation: 1, stateDirectory: join(dir, "state"),
    control: { kind: "unix", path: socket },
    runtime: { kind: "kubernetes", kubeconfig: file("k3s/kubeconfig.yaml"), context: "webdock-dev", helm: "/usr/local/bin/helm", chart, chartSha256: chartDigest(chart) },
    workloads: true,
  }), { mode: 0o600 });
  return cluster;
}

function seed() {
  waitForDatabase();
  mkdirSync(plugins, { recursive: true, mode: 0o700 });
  if (!existsSync(pluginFile("invocation.key"))) {
    const keys = generateKeyPairSync("ed25519");
    const raw = (part) => Buffer.from(keys.privateKey.export({ format: "jwk" })[part], "base64url").toString("hex");
    writeFileSync(pluginFile("invocation.key"), raw("d"), { mode: 0o600 });
    writeFileSync(pluginFile("invocation.pub"), raw("x"), { mode: 0o600 });
    writeFileSync(pluginFile("credential"), secret(), { mode: 0o600 });
  }
  psql(`BEGIN;
SELECT set_config('webdock.organization_allocation', json_build_object('customer_id', :'customer', 'organization_id', :'organization')::text, true);
INSERT INTO webdock_admin.customers(id,name) SELECT :'customer','Demo customer' WHERE NOT EXISTS(SELECT 1 FROM webdock_admin.customers WHERE id=:'customer');
INSERT INTO webdock_admin.projects(id,name,customer_id) SELECT :'project','Demo project',:'customer' WHERE NOT EXISTS(SELECT 1 FROM webdock_admin.projects WHERE id=:'project');
INSERT INTO webdock_auth.plugin_bootstrap(singleton,mode) VALUES(true,'fresh') ON CONFLICT DO NOTHING;
COMMIT;`, demo);
  const credential = createHash("sha256").update(readFileSync(pluginFile("credential"), "utf8")).digest("hex");
  for (const id of Object.keys(PLUGINS)) seedPlugin(id, credential);
  const cluster = agent();
  const runtimes = (entry) => Object.fromEntries(Object.keys(PLUGINS).map((id) => [id, entry(id)]));
  writeFileSync(pluginFile("host-bindings.json"), JSON.stringify({ runtimes: runtimes((id) => ({
    generation: 1, namespace: "local", service: id, credentialSha256: credential, peers: [{ kind: "unix", uid }],
    ...(PLUGINS[id].capabilities?.includes("plugin.workloads") ? { workloadCluster: cluster } : {}),
    ...(PLUGINS[id].routeDomains ? { routeDomains: PLUGINS[id].routeDomains } : {}),
  })) }), { mode: 0o600 });
  writeFileSync(pluginFile("connections.json"), JSON.stringify({ runtimes: runtimes((id) => ({
    generation: 1, transport: { kind: "unix", path: pluginFile(`${id}.sock`) },
  })) }), { mode: 0o600 });
  writeFileSync(pluginFile("plugin-config.json"), JSON.stringify({
    hostBindings: pluginFile("host-bindings.json"), invocationSigningKey: pluginFile("invocation.key"),
    connections: pluginFile("connections.json"), control: { kind: "unix", path: pluginFile("control.sock") },
  }), { mode: 0o600 });
  demoOwner();
  console.log(`Seeded Demo customer ${demo.customer} / Demo project ${demo.project} with ${Object.keys(PLUGINS).join(", ")}. Customer sign-in: .dev-local/demo-owner.json`);
}

function runtimeEnv(value) {
  const bootstrap = JSON.parse(readFileSync(file("initial-secrets.json"), "utf8"));
  return {
    PATH: process.env.PATH, HOME: process.env.HOME, NODE_ENV: "development", RUST_LOG: process.env.RUST_LOG ?? "info",
    ...bootstrap.environment, ...origins,
    WEBDOCK_DATABASE_URL: url("webdock_auth_bootstrap", value.roles.webdock_auth_bootstrap),
    WEBDOCK_SESSION_DATABASE_URL: url("webdock_auth_runtime", value.roles.webdock_auth_runtime),
    WEBDOCK_REGISTRY_DATABASE_URL: url("webdock_registry_runtime", value.roles.webdock_registry_runtime),
    WEBDOCK_WORKER_DATABASE_URL: url("webdock_worker_runtime", value.roles.webdock_worker_runtime),
    WEBDOCK_AUTH_SECRET: value.auth, DATABASE_RLS_SECRET: value.rls,
    WEBDOCK_FEATURES: process.env.WEBDOCK_DEV_FEATURES ?? "{}",
    WEBDOCK_BIND_ADDRESS: "127.0.0.1", PORT: "3130",
    WEBDOCK_ACCOUNT_SMTP: `127.0.0.1:${process.env.WEBDOCK_DEV_SMTP_PORT ?? 55025}`,
    ...(existsSync(pluginFile("plugin-config.json")) ? { WEBDOCK_NATIVE_PLUGIN_CONFIG: pluginFile("plugin-config.json") } : {}),
  };
}
// Tilt execs the binaries directly with this file, so stopping Tilt never orphans a server.
function writeEnv(value) {
  const quote = (text) => `'${String(text).replaceAll("'", "'\\''")}'`;
  const lines = Object.entries(runtimeEnv(value)).filter(([key]) => !["PATH", "HOME"].includes(key)).map(([key, text]) => `${key}=${quote(text)}`);
  writeFileSync(file("runtime.env"), lines.join("\n") + "\n", { mode: 0o600 });
}

const command = process.argv[2];
const value = secrets();
if (command === "init") init(value);
else if (command === "plugin-image") await pluginImage(process.argv[3] ?? "echo-demo");
else if (command === "seed") { seed(); writeEnv(value); }
else if (command === "kubeconfig") kubeconfig();
else if (command === "cluster-ca") clusterCa();
else if (command !== "secrets") throw new Error("Use: secrets | init | plugin-image | seed | kubeconfig | cluster-ca");
