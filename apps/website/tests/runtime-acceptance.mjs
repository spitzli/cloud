// Actual Vinext build/dev servers, a disposable local database and an HTTPS test IdP.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { randomBytes, createHash } from 'node:crypto';
import { mkdtemp, writeFile, readFile, rm, chmod, mkdir, copyFile, lstat } from 'node:fs/promises';
import { join, dirname, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer } from 'node:net';
import { once } from 'node:events';
import { Pool } from 'pg';
import { generateKeyPair, exportJWK, SignJWT } from 'jose';
import { chromium } from 'playwright';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const mode = process.argv[2] || 'vercel';
assert.ok(['vercel', 'bun', 'dev'].includes(mode));
const scheme = mode === 'dev' ? 'http' : 'https';
const directory = await mkdtemp(join(root, '.runtime-test/vinext-site-'));
await chmod(directory, 0o700);
let application = join(root, 'apps/website');
if (mode === 'dev') {
  // Payload generates types in development. Keep those writes in this owned source snapshot.
  application = join(directory, 'application');
  const files = execFileSync('git', ['ls-files', '--cached', '--others', '--exclude-standard', '-z', '--', 'apps/website'], { cwd: root, encoding: 'utf8' }).split('\0').filter(Boolean);
  for (const file of files) {
    const name = relative('apps/website', file);
    if (name.startsWith('.env')) continue;
    const source = join(root, file), target = join(application, name);
    let info;
    try { info = await lstat(source); } catch (error) { if (error.code === 'ENOENT') continue; throw error; }
    assert.ok(info.isFile() && !info.isSymbolicLink());
    await mkdir(dirname(target), { recursive: true });
    await copyFile(source, target);
    assert.ok((await readFile(source)).equals(await readFile(target)), 'Dev source snapshot must preserve exact file bytes.');
  }
}
const runID = randomBytes(8).toString('hex');
const container = `spitzli-vinext-site-${runID}`;
const label = `dev.spitzli.vinext=${runID}`;
const privateValues = Array.from({ length: 5 }, () => randomBytes(32).toString('hex'));
const [databasePassword, payloadSecret, clientSecret, cookieSecret, rootPassword] = privateValues;
const clean = { PATH: process.env.PATH, NODE_ENV: 'production' };
const children = new Set();
const pools = [];
let created = false, provider, browser, completed = false;
const redact = text => privateValues.reduce((result, value) => result.replaceAll(value, '[redacted]'), text);
const docker = (args, input) => execFileSync('docker', args, { encoding: 'utf8', input, stdio: ['pipe', 'pipe', 'pipe'] }).trim();
async function child(name, args, env = clean) {
  const process = spawn(Bun.which('bun'), ['--no-env-file', ...args], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
  children.add(process);
  let log = '';
  process.stdout.on('data', chunk => { log += chunk; });
  process.stderr.on('data', chunk => { log += chunk; });
  const done = once(process, 'exit').then(async ([code]) => {
    children.delete(process);
    await writeFile(join(directory, `${name}.log`), redact(log), { mode: 0o600 });
    return code;
  });
  return { process, done, output: () => log };
}
async function run(name, args, env) {
  const operation = await child(name, args, env);
  const timeout = setTimeout(() => operation.process.kill('SIGKILL'), 120_000);
  try { assert.equal(await operation.done, 0, `${name} failed; redacted logs: ${directory}`); }
  finally { clearTimeout(timeout); }
}
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function freePort() {
  const reserve = createServer(); reserve.listen(0, '127.0.0.1'); await once(reserve, 'listening');
  const port = reserve.address().port; await new Promise(resolve => reserve.close(resolve)); return port;
}

try {
  const endpoint = process.env.DOCKER_HOST || docker(['context', 'inspect', '--format', '{{.Endpoints.docker.Host}}']);
  assert.ok(endpoint.startsWith('unix://'), 'Only a local Docker engine may host this fixture.');
  if (mode === 'vercel') {
    const config = JSON.parse(await readFile(join(root, 'apps/website/.vercel/output/functions/__server.func/.vc-config.json'), 'utf8'));
    assert.equal(config.runtime, 'bun1.4.x');
    assert.deepEqual(config.regions, ['fra1']);
  }
  const databasePort = await freePort();
  // A host-loopback listener avoids Docker NIC changes interrupting Chromium module imports.
  docker(['run', '-d', '--name', container, '--label', label, '--network', 'host', '-e', `POSTGRES_PASSWORD=${rootPassword}`, '--memory', '512m', '--cpus', '1', 'postgres:18.6', '-c', 'listen_addresses=127.0.0.1', '-c', `port=${databasePort}`]);
  created = true;
  const database = new Pool({ host: '127.0.0.1', port: databasePort, user: 'postgres', password: rootPassword, max: 1, connectionTimeoutMillis: 1000 });
  pools.push(database);
  for (let attempt = 0; ; attempt++) {
    try { await database.query('SELECT 1'); break; }
    catch (error) { if (attempt === 100) throw error; await pause(100); }
  }
  await database.query(`CREATE ROLE fixture_migrate LOGIN PASSWORD '${databasePassword}'; CREATE ROLE fixture_runtime LOGIN PASSWORD '${databasePassword}';`);
  await database.query('CREATE DATABASE website OWNER fixture_migrate');
  const scoped = new Pool({ host: '127.0.0.1', port: databasePort, user: 'postgres', password: rootPassword, database: 'website', max: 1 });
  pools.push(scoped);
  await scoped.query('CREATE SCHEMA webdock AUTHORIZATION fixture_migrate; REVOKE CREATE ON SCHEMA public FROM PUBLIC;');
  const cert = join(directory, 'cert.pem'), key = join(directory, 'key.pem');
  execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', key, '-out', cert, '-days', '1', '-subj', '/CN=localhost', '-addext', 'subjectAltName=DNS:localhost,IP:127.0.0.1'], { stdio: 'ignore' });
  await chmod(key, 0o600);
  const tls = { cert: Bun.file(cert), key: Bun.file(key) };
  const { publicKey, privateKey } = await generateKeyPair('RS256', { extractable: true });
  const jwk = { ...await exportJWK(publicKey), kid: 'owned-vinext', alg: 'RS256', use: 'sig' };
  const codes = new Map(), tokens = new Map();
  let currentRole = 'operator', revoked = false, introspections = 0;
  let issuer, appOrigin;
  provider = Bun.serve({ hostname: '127.0.0.1', port: 0, tls: mode === 'dev' ? undefined : tls, async fetch(request) {
    const url = new URL(request.url);
    if (url.pathname.includes('.well-known')) return Response.json({ issuer, authorization_endpoint: `${issuer}/oauth2/authorize`, token_endpoint: `${issuer}/oauth2/token`, introspection_endpoint: `${issuer}/oauth2/introspect`, jwks_uri: `${issuer}/jwks`, response_types_supported: ['code'], subject_types_supported: ['public'], id_token_signing_alg_values_supported: ['RS256'], token_endpoint_auth_methods_supported: ['client_secret_post'], introspection_endpoint_auth_methods_supported: ['client_secret_post'], code_challenge_methods_supported: ['S256'] });
    if (url.pathname.endsWith('/jwks')) return Response.json({ keys: [jwk] });
    if (url.pathname.endsWith('/oauth2/authorize')) {
      assert.equal(url.searchParams.get('client_id'), 'vinext-fixture');
      assert.equal(url.searchParams.get('redirect_uri'), `${appOrigin}/api/sso/callback`);
      assert.equal(url.searchParams.get('code_challenge_method'), 'S256');
      const code = randomBytes(24).toString('hex');
      codes.set(code, { nonce: url.searchParams.get('nonce'), challenge: url.searchParams.get('code_challenge') });
      return Response.redirect(`${appOrigin}/api/sso/callback?${new URLSearchParams({ code, state: url.searchParams.get('state') })}`, 302);
    }
    const form = new URLSearchParams(await request.text());
    assert.equal(form.get('client_id'), 'vinext-fixture');
    assert.equal(form.get('client_secret'), clientSecret);
    if (url.pathname.endsWith('/oauth2/token')) {
      const code = form.get('code'), flow = codes.get(code);
      assert.ok(flow); codes.delete(code);
      assert.equal(createHash('sha256').update(form.get('code_verifier')).digest('base64url'), flow.challenge);
      const accessToken = randomBytes(24).toString('hex'); tokens.set(accessToken, true);
      const idToken = await new SignJWT({ nonce: flow.nonce }).setProtectedHeader({ alg: 'RS256', kid: jwk.kid }).setIssuer(issuer).setAudience('vinext-fixture').setSubject('vinext-operator').setIssuedAt().setExpirationTime('5m').sign(privateKey);
      return Response.json({ access_token: accessToken, token_type: 'Bearer', expires_in: 300, id_token: idToken });
    }
    if (url.pathname.endsWith('/oauth2/introspect')) {
      introspections++;
      return Response.json({ active: !revoked && tokens.has(form.get('token')), client_id: 'vinext-fixture', sub: 'vinext-operator', exp: Math.floor(Date.now() / 1000) + 300, webdock_role: currentRole });
    }
    return new Response(null, { status: 404 });
  } });
  issuer = `${scheme}://127.0.0.1:${provider.port}/api/auth`;
  const appPort = await freePort();
  appOrigin = `${scheme}://127.0.0.1:${appPort}`;
  const databaseURL = role => `postgresql://${role}:${databasePassword}@127.0.0.1:${databasePort}/website`;
  const env = { ...clean, PAYLOAD_SECRET: payloadSecret, NEXT_PUBLIC_SERVER_URL: appOrigin, SPITZLI_AUTH_ISSUER: issuer, SPITZLI_SSO_CLIENT_ID: 'vinext-fixture', SPITZLI_SSO_CLIENT_SECRET: clientSecret, SPITZLI_SSO_COOKIE_SECRET: cookieSecret, SPITZLI_SSO_APP_ORIGIN: appOrigin, SPITZLI_SSO_ENFORCE: 'true', NODE_EXTRA_CA_CERTS: cert, DATABASE_URL: databaseURL('fixture_runtime') };
  const seed = join(directory, 'seed.mjs');
  await writeFile(seed, `import {getPayload} from 'payload';
const config=(await import(${JSON.stringify(join(root, 'apps/website/src/payload.config.ts'))})).default;
const {migrations}=await import(${JSON.stringify(join(root, 'apps/website/src/migrations-instance/index.ts'))});
const {landingDefaults}=await import(${JSON.stringify(join(root, 'apps/website/src/cms/landing.ts'))});
const payload=await getPayload({config,disableOnInit:true});
try { const result=await payload.db.migrate({migrations,shouldPrompt:false});if(result?.cancelled)throw Error('Cancelled migration');
await payload.create({collection:'users',overrideAccess:true,context:{bootstrap:true},data:{email:'operator@example.invalid',name:'Vinext Operator',role:'operator',authSubject:'vinext-operator'}});
await payload.updateGlobal({slug:'landing-page',overrideAccess:true,data:landingDefaults});
}finally{await payload.destroy();}
`, { mode: 0o600 });
  await run('migrate', [seed], { ...env, SPITZLI_SSO_CLIENT_ID: undefined, PAYLOAD_MIGRATING: 'true', DATABASE_URL: undefined, DATABASE_URL_UNPOOLED: databaseURL('fixture_migrate') });
  await scoped.query('GRANT USAGE ON SCHEMA webdock TO fixture_runtime; GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA webdock TO fixture_runtime; GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA webdock TO fixture_runtime;');
  const server = join(directory, 'server.mjs');
  await writeFile(server, mode === 'bun' ? `await import(${JSON.stringify(join(root, 'apps/website/.output/server/index.mjs'))});console.log('Owned Vinext ready');` : `import {resolve,sep} from 'node:path';
import handler from ${JSON.stringify(join(root, 'apps/website/.vercel/output/functions/__server.func/index.mjs'))};
const staticRoot=${JSON.stringify(join(root, 'apps/website/.vercel/output/static'))};
Bun.serve({hostname:'127.0.0.1',port:${appPort},tls:{cert:Bun.file(${JSON.stringify(cert)}),key:Bun.file(${JSON.stringify(key)})},async fetch(request){
const url=new URL(request.url);const path=resolve(staticRoot,'.'+decodeURIComponent(url.pathname));
if(path.startsWith(staticRoot+sep)&&await Bun.file(path).exists())return new Response(Bun.file(path));
return handler.fetch(request,{waitUntil:promise=>promise.catch(()=>{})});}});
console.log('Owned Vinext ready');
`, { mode: 0o600 });
  let args = [server];
  if (mode === 'dev') {
    const configFile = join(directory, 'vite.config.mts');
    await writeFile(configFile, `export {default} from ${JSON.stringify(join(application, 'vite.config.ts'))};`, { mode: 0o600 });
    args = ['--cwd', application, '--bun', join(root, 'node_modules/vite/bin/vite.js'), '--config', configFile, '--host', '127.0.0.1', '--port', String(appPort), '--strictPort'];
  }
  const serving = await child('server', args, { ...env, NITRO_SSL_CERT: cert, NITRO_SSL_KEY: key, HOST: '127.0.0.1', PORT: String(appPort), SPITZLI_SSO_ALLOW_LOCAL_HTTP: mode === 'dev' ? 'true' : 'false', NODE_ENV: mode === 'dev' ? 'development' : 'production' });
  for (let attempt = 0; !serving.output().includes(mode === 'dev' ? 'Local:' : 'Owned Vinext ready'); attempt++) {
    assert.equal(serving.process.exitCode, null, `Built server exited: ${directory}`);
    assert.ok(attempt < 400, 'Server startup exceeded 40 seconds'); await pause(100);
  }
  browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
  const context = await browser.newContext({ ignoreHTTPSErrors: true, locale: 'en-GB' });
  const external = [];
  await context.route('**/*', route => {
    const url = new URL(route.request().url());
    if ([appOrigin, new URL(issuer).origin].includes(url.origin)) return route.continue();
    external.push(url.origin + url.pathname);
    return route.abort();
  });
  const page = await context.newPage();
  const errors = [], failed = [], consoleErrors = [], posts = [], networkErrors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') consoleErrors.push(message.text()); });
  page.on('response', async response => { if (response.request().method() === 'POST') posts.push({ path: new URL(response.url()).pathname, status: response.status(), ...(response.status() >= 400 ? { body: (await response.text().catch(() => '')).slice(0, 1000) } : {}) }); });
  page.on('requestfailed', request => networkErrors.push({ path: new URL(request.url()).pathname, error: request.failure()?.errorText }));
  page.on('response', response => { if (response.status() >= 500) failed.push(`${response.status()} ${new URL(response.url()).pathname}`); });
  for (const path of ['/', '/terms', '/privacy', '/imprint', '/integrations/vercel', '/robots.txt', '/sitemap.xml', '/icon.svg', '/og']) {
    const response = await context.request.get(appOrigin + path);
    assert.equal(response.status(), 200, path); const bytes = await response.body(); assert.ok(bytes.length > 30, path);
    if (path === '/og') {
      assert.match(response.headers()['content-type'], /^image\/png/);
      assert.deepEqual([...bytes.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
      assert.equal(bytes.readUInt32BE(16), 1200); assert.equal(bytes.readUInt32BE(20), 630);
    }
  }
  assert.equal((await context.request.get(`${appOrigin}/api/cms`)).status(), 401);
  assert.equal((await context.request.get(`${appOrigin}/api/users`)).status(), 403);
  await page.goto(appOrigin); await page.getByRole('heading', { level: 1 }).waitFor();
  await page.waitForLoadState('networkidle');
  assert.ok(await page.evaluate(async () => { await document.fonts.ready; return [...document.fonts].some(font => font.status === 'loaded'); }), 'Self-hosted fonts must load.');
  for (const width of [320, 768, 1280]) {
    await page.setViewportSize({ width, height: 900 });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), `${width}: horizontal overflow`);
  }
  await page.goto(`${appOrigin}/api/sso/login?returnTo=/cms`);
  await page.waitForURL(`${appOrigin}/cms`);
  await page.getByText('Website content', { exact: true }).first().waitFor();
  const session = await context.request.get(`${appOrigin}/api/cms`);
  assert.equal(session.status(), 200); const cms = await session.json();
  assert.match(session.headers()['cache-control'], /no-store/);
  assert.equal(cms.user.role, 'operator');
  const access = await context.request.post(`${appOrigin}/api/globals/landing-page/access`, { headers: { origin: appOrigin }, data: {} });
  assert.equal(access.status(), 200, `Global field permissions: ${await access.text()}`);
  await page.goto(`${appOrigin}/system/globals/landing-page`);
  const title = page.getByRole('textbox', { name: /^Page title/ });
  try {
    await title.waitFor();
    await title.fill('Vinext verified content');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await page.getByText(/successfully/i).first().waitFor();
  }
  catch (error) {
    await writeFile(join(directory, 'editor.json'), JSON.stringify({ url: page.url(), text: await page.locator('body').innerText(), errors, consoleErrors, posts, failed, external, networkErrors, fields: await page.locator('input').evaluateAll(inputs => inputs.map(input => ({ name: input.name, id: input.id, placeholder: input.placeholder, disabled: input.disabled }))) }, null, 2), { mode: 0o600 });
    await page.screenshot({ path: join(directory, 'editor.png'), fullPage: true });
    throw error;
  }
  const saved = await scoped.query('SELECT seo_title FROM webdock.landing_page');
  assert.equal(saved.rows[0].seo_title, 'Vinext verified content');
  const html = await (await context.request.get(appOrigin)).text();
  assert.match(html, /Vinext verified content/);
  currentRole = 'reader';
  assert.equal((await (await context.request.get(`${appOrigin}/api/cms`)).json()).user.role, 'reader');
  const denied = await context.request.post(`${appOrigin}/api/cms`, { headers: { origin: appOrigin }, data: { module: 'landing-page', id: 'global', data: { seoTitle: 'Denied' } } });
  assert.equal(denied.status(), 403);
  revoked = true;
  assert.equal((await context.request.get(`${appOrigin}/api/cms`)).status(), 401);
  assert.ok(introspections >= 4, 'Permissions must be introspected live.');
  assert.deepEqual(failed, []); assert.deepEqual(errors, []); assert.deepEqual(external, []);
  await context.close();
  completed = true;
  console.log(`Vinext website ${mode} PASS: fresh DB and separate roles, public routes/SEO/fonts, responsive hydration, ${scheme.toUpperCase()} OIDC/PKCE, Payload editor save/readback, role downgrade and revocation.`);
} finally {
  await browser?.close();
  for (const process of children) process.kill('SIGTERM');
  for (const process of children) if (process.exitCode === null) await once(process, 'exit');
  provider?.stop(true);
  await Promise.all(pools.map(pool => pool.end()));
  if (created) {
    const actual = docker(['inspect', '--format', '{{index .Config.Labels "dev.spitzli.vinext"}}', container]);
    assert.equal(actual, runID, 'Cleanup requires exact fixture ownership.');
    docker(['rm', '--force', '--volumes', container]);
  }
  if (completed) await rm(directory, { recursive: true });
  else console.error(`Owned fixture removed; redacted diagnostics retained in ${directory}`);
}
