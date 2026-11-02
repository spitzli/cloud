import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../..', import.meta.url));
const fixture = await mkdtemp(join(tmpdir(), 'webdock-static-core-'));
try {
  const entry = join(fixture, 'entry.ts');
  await writeFile(entry, `
import { createApp } from ${JSON.stringify(resolve(root, 'legacy/control-plane/src/app.ts'))};
import { registryProtocol } from ${JSON.stringify(resolve(root, 'legacy/control-plane/src/protocols/registry.ts'))};
import { readConfig } from ${JSON.stringify(resolve(root, 'legacy/control-plane/src/config.ts'))};
import { closeDatabases } from ${JSON.stringify(resolve(root, 'legacy/control-plane/src/db/runtime.ts'))};
const config = readConfig(process.env);
const registry = await registryProtocol(config);
const paths = registry.app.routes.map(route => route.method + ' ' + route.path);
if (paths.length !== 5 || !paths.includes('GET /v1/operator/registry/:collection') || !paths.includes('PATCH /v1/operator/registry/:collection/:id')) throw Error(JSON.stringify(paths));
const app = await createApp(config, { mounts: [registry], ready: async () => {} });
for (const [path, status] of [['/healthz', 200], ['/readyz', 200], ['/v1/operator/registry/customers', 404]]) {
  const response = await app.handle(new Request(config.apiOrigin + path));
  if (response.status !== status) throw Error(path + ': ' + response.status);
}
await closeDatabases();
console.log('Bundled core health, readiness and guarded registry routes execute without source route directories.');
`);
  const builder = join(fixture, 'build.ts');
  await writeFile(builder, `const result = await Bun.build({entrypoints:[${JSON.stringify(entry)}],target:'bun',outdir:${JSON.stringify(join(fixture,'dist'))},plugins:[{name:'deny-products',setup(build){build.onResolve({filter:/^@webdock\\/(?:module-(?:mail|hosting|databases|storage|cms)(?:\\/|$)|adapter-(?!database-postgres(?:\\/|$))|hosting-|mail-(?:contracts|core)(?:\\/|$)|database-contracts(?:\\/|$)|payload-|page-builder(?:\\/|$))/},({path,importer})=>{throw Error('Forbidden '+path+' from '+importer)});}}]}); if(!result.success){for(const log of result.logs)console.error(log);process.exit(1);}`);
  const bun = process.env.BUN_BINARY || process.execPath;
  const built = spawnSync(bun, ['--no-env-file', builder], {cwd: fixture,encoding:'utf8'});
  assert.equal(built.status, 0, built.stderr);
  const run = spawnSync(bun, ['--no-env-file', join(fixture,'dist/entry.js')], {cwd:fixture,encoding:'utf8',timeout:30000,env:{...process.env,DATABASE_URL:'postgres://fixture:fixture@127.0.0.1:1/fixture',WEBDOCK_MODULES:JSON.stringify({configVersion:1,modules:{}}),WEBDOCK_AUTH_PROVIDER:JSON.stringify({provider:'clerk',issuer:'https://clerk.example.test',authorizedParties:['https://console.example.test'],audience:'console'}),WEBDOCK_OAUTH_PROVIDER:'disabled',WEBDOCK_API_DOMAINS:'auth,portal,registry'}});
  assert.equal(run.status, 0, run.stderr || run.stdout);
  console.log(run.stdout.trim());
} finally { await rm(fixture,{recursive:true,force:true}); }
