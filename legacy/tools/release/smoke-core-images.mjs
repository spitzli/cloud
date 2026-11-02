/** Exact-image packaging smoke. No public ingress, browser session or SMTP acceptance is claimed. */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const tags = process.argv.slice(2);
if (tags.length !== 4 || tags.some(tag => !tag || tag.startsWith('-')))
  throw Error('Usage: smoke-core-images.mjs core-api-tag core-console-tag auth-tag migrate-tag');
const endpoint = execFileSync('docker', ['context', 'inspect', '--format', '{{.Endpoints.docker.Host}}'], { encoding: 'utf8' }).trim();
if (!endpoint.startsWith('unix://')) throw Error('A local Docker engine is required');
const dockerEnv = { PATH: process.env.PATH, HOME: process.env.HOME, DOCKER_HOST: endpoint };
function docker(args, env = {}, timeout = 120000) {
  return execFileSync('docker', args, { env: { ...dockerEnv, ...env }, encoding: 'utf8', timeout, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}
// Resolve once: later tag changes must not change the images under test.
const [apiImage, consoleImage, authImage, migrateImage] = tags.map(tag => docker(['image', 'inspect', tag, '--format', '{{.Id}}']));
const prefix = 'webdock-core-smoke-' + randomBytes(8).toString('hex');
const label = 'webdock.release-smoke';
const directory = await mkdtemp(join(tmpdir(), prefix + '-'));
const containers = [];
let networkCreated = false;
let phase = 'fixture';
const restrictions = ['--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--tmpfs', '/tmp:rw,nosuid,size=64m,mode=1777'];
const secret = () => randomBytes(48).toString('hex');
const environmentArgs = env => Object.keys(env).flatMap(key => ['--env', key]);
const ownedArgs = name => ['--name', name, '--label', `${label}=${prefix}`];
async function poll(check, description) {
  for (let attempt = 0; attempt < 90; attempt++) {
    try { if (check()) return; } catch {}
    await new Promise(resolve => setTimeout(resolve, 500));
  }
  throw Error(`${description} did not become ready`);
}
function runMigration(name, env, args) {
  const container = `${prefix}-${name}`;
  containers.push(container);
  return docker(['run', '--rm', ...ownedArgs(container), '--network', prefix, ...restrictions,
    '--user', `${process.getuid()}:${process.getgid()}`, '--workdir', '/app',
    '--volume', `${directory}:/fixture`, ...environmentArgs(env), '--entrypoint', 'bun', migrateImage,
    '--no-env-file', ...args], env);
}
try {
  docker(['network', 'create', '--internal', '--label', `${label}=${prefix}`, prefix]);
  networkCreated = true;
  execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', join(directory, 'key.pem'),
    '-out', join(directory, 'cert.pem'), '-days', '1', '-subj', '/CN=database', '-addext', 'subjectAltName=DNS:database'], { stdio: 'ignore' });
  const ca = await readFile(join(directory, 'cert.pem'), 'utf8');
  const database = `${prefix}-database`;
  containers.push(database);
  const postgresEnv = { POSTGRES_PASSWORD: secret(), POSTGRES_DB: 'webdock_release_test' };
  docker(['run', '-d', ...ownedArgs(database), '--network', prefix, '--network-alias', 'database',
    '--volume', `${directory}:/fixture:ro`, ...environmentArgs(postgresEnv), '--entrypoint', 'sh', 'postgres:18-alpine', '-c',
    'cp /fixture/key.pem /tmp/server.key && cp /fixture/cert.pem /tmp/server.crt && chown postgres:postgres /tmp/server.key /tmp/server.crt && chmod 600 /tmp/server.key && exec docker-entrypoint.sh postgres -c ssl=on -c ssl_key_file=/tmp/server.key -c ssl_cert_file=/tmp/server.crt'], postgresEnv);
  // The entrypoint's temporary initialization server accepts Unix sockets before TCP is ready.
  await poll(() => docker(['exec', database, 'pg_isready', '-h', '127.0.0.1', '-U', 'postgres', '-d', postgresEnv.POSTGRES_DB]).includes('accepting connections'), 'PostgreSQL 18');

  const roles = ['webdock_auth_bootstrap', 'webdock_auth_runtime', 'webdock_worker_runtime', 'webdock_admin_runtime',
    'webdock_registry_runtime', 'webdock_runtime', 'webdock_browser_website', 'webdock_cms_archive_runtime', 'webdock_offline_migrate'];
  const passwords = Object.fromEntries(roles.map(role => [role, secret()]));
  const roleURL = (role, password = passwords[role]) => `postgresql://${role}:${password}@database:5432/${postgresEnv.POSTGRES_DB}`;
  const target = `database:5432/${postgresEnv.POSTGRES_DB}`;
  const origins = { BETTER_AUTH_URL: 'https://auth.release.example.test', WEBDOCK_STUDIO_ORIGIN: 'https://studio.release.example.test',
    WEBDOCK_CONSOLE_ORIGIN: 'https://console.release.example.test', WEBDOCK_API_ORIGIN: 'https://api.release.example.test' };
  const offline = { DATABASE_URL_UNPOOLED: roleURL('postgres', postgresEnv.POSTGRES_PASSWORD), DATABASE_CONTEXT_SECRET: secret(), DATABASE_CA_CERT: ca };
  phase = 'empty database migration';
  runMigration('install', { ...offline, WEBDOCK_BOOTSTRAP_EMPTY_DATABASE: 'true', MIGRATION_ROLE_PASSWORDS: JSON.stringify(passwords) }, ['tools/migrations/install.ts', target]);
  offline.DATABASE_URL_UNPOOLED = roleURL('webdock_offline_migrate');
  phase = 'empty plugin initialization';
  runMigration('plugins', { ...offline, WEBDOCK_MODULES: '{"configVersion":1,"modules":{}}' }, ['tools/plugins/bootstrap.ts', target, 'fresh']);
  phase = 'initial operator';
  await writeFile(join(directory, 'operator-input.json'), JSON.stringify({ email: 'operator@example.test', name: 'Disposable release operator',
    password: secret(), authOrigin: origins.BETTER_AUTH_URL, studioOrigin: origins.WEBDOCK_STUDIO_ORIGIN, consoleOrigin: origins.WEBDOCK_CONSOLE_ORIGIN }), { mode: 0o600 });
  runMigration('operator', { ...offline, ...origins }, ['tools/migrations/operator.ts', target, '/fixture/operator-input.json', '/fixture/operator-output.json']);
  const operator = JSON.parse(await readFile(join(directory, 'operator-output.json'), 'utf8'));
  assert.ok(operator.operatorID && operator.clientID && operator.environment.WEBDOCK_SSO_CLIENT_SECRET);
  phase = 'core API production readiness';
  const runtime = { ...origins, ...operator.environment, NODE_ENV: 'production', DATABASE_URL: roleURL('webdock_auth_runtime'),
    AUTH_DATABASE_URL: roleURL('webdock_auth_bootstrap'), DATABASE_WORKER_URL: roleURL('webdock_worker_runtime'),
    REGISTRY_DATABASE_URL: roleURL('webdock_registry_runtime'), DATABASE_CA_CERT: ca, DATABASE_POOL_MAX: '2', REGISTRY_POOL_MAX: '2',
    DATABASE_RLS_SECRET: offline.DATABASE_CONTEXT_SECRET, BETTER_AUTH_SECRET: secret(), WEBDOCK_AUTH_PROVIDER: '{"provider":"better-auth"}',
    WEBDOCK_OAUTH_PROVIDER: 'better-auth', WEBDOCK_API_DOMAINS: 'auth,portal,registry', WEBDOCK_MODULES: '{"configVersion":1,"modules":{}}',
    WEBDOCK_INGRESS_SECRET: secret(), WEBDOCK_TRUSTED_PROXIES: '127.0.0.1/32' };
  const api = `${prefix}-api`;
  containers.push(api);
  docker(['run', '-d', ...ownedArgs(api), '--network', prefix, ...restrictions, ...environmentArgs(runtime), apiImage], runtime);
  await poll(() => docker(['exec', api, 'bun', '-e', "if(!(await fetch('http://127.0.0.1:3130/readyz')).ok)process.exit(1)"]) === '', 'Core API /readyz');
  // Queries use the fixture administrator only inside its owned database container.
  const state = docker(['exec', database, 'psql', '-U', 'postgres', '-d', postgresEnv.POSTGRES_DB, '-At', '-c',
    `SELECT (SELECT count(*) FROM webdock_auth.plugin_bootstrap), (SELECT count(*) FROM webdock_auth.plugin_installation), (SELECT count(*) FROM webdock_auth."user" WHERE role='operator' AND "mustChangePassword" AND NOT "twoFactorEnabled"), (SELECT count(*) FROM webdock_auth.session), (SELECT bool_and(ssl) FROM pg_stat_ssl JOIN pg_stat_activity USING(pid) WHERE usename IN ('webdock_auth_runtime','webdock_auth_bootstrap','webdock_worker_runtime','webdock_registry_runtime')), (SELECT settings->>'supportEmail'='operator@example.test' AND settings->>'cmsEnabled'='false' FROM webdock_auth.platform_settings WHERE id);`]);
  assert.equal(state, '1|0|1|0|t|t', 'Fresh core requires empty plugins, unsecured operator, no session, TLS runtime connections and operator-owned branding');
  for (const [name, image, app, port] of [['console', consoleImage, '.core-build/console', 3120], ['auth', authImage, 'apps/auth', 3125]]) {
    phase = `${name} image assets`;
    const container = `${prefix}-${name}`;
    containers.push(container);
    const env = { ...origins, WEBDOCK_AUTH_PROVIDER: '{"provider":"better-auth"}' };
    docker(['run', '-d', ...ownedArgs(container), '--network', 'none', ...restrictions,
      '--tmpfs', `/app/${app}/.next/cache:rw,nosuid,size=128m,mode=1777`, ...environmentArgs(env), image], env);
    const code = `const fs=require('node:fs');const file=fs.readdirSync('/app/${app}/.next/static/chunks').find(f=>f.endsWith('.js'));if(!file)process.exit(1);fetch('http://127.0.0.1:${port}/_next/static/chunks/'+file).then(r=>{if(r.status!==200)process.exit(1)}).catch(()=>process.exit(1));`;
    await poll(() => docker(['exec', container, 'node', '-e', code]) === '', `${name} static asset`);
  }
  console.log(JSON.stringify({ check: 'core-images-fresh-installation', passed: true, images: { api: apiImage, console: consoleImage, auth: authImage, migrate: migrateImage },
    database: 'PostgreSQL 18 with verified TLS', emptyPlugins: true, offlineOperator: true, productionAPIReadiness: true, frontendAssets: true,
    browserMFA: false, publicHTTPSIngress: false, smtpDelivery: false }));
} catch (error) {
  // Never emit Docker environments, installer input or generated credentials on failure.
  throw new Error(`Core image smoke failed during ${phase}${error instanceof assert.AssertionError ? ': ' + error.message : ''}`);
} finally {
  for (const container of containers.reverse()) {
    let owner;
    try { owner = docker(['inspect', '--format', `{{index .Config.Labels "${label}"}}`, container]); } catch { continue; }
    if (owner !== prefix) throw Error('Refusing cleanup of a container not owned by this smoke');
    docker(['rm', '-fv', container]);
  }
  if (networkCreated) {
    const owner = docker(['network', 'inspect', '--format', `{{index .Labels "${label}"}}`, prefix]);
    if (owner !== prefix) throw Error('Refusing cleanup of a network not owned by this smoke');
    docker(['network', 'rm', prefix]);
  }
  await rm(directory, { recursive: true, force: true });
}
