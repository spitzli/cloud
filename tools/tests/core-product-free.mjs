import assert from 'node:assert/strict';
import { cp, mkdir, mkdtemp, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

// Build a copied core package in an owned fixture, rejecting product dependencies
// during resolution even when a third-party symlink leads back to the workspace.
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const bun = process.env.BUN_BINARY || process.execPath;
const fixture = await mkdtemp(join(tmpdir(), 'webdock-core-product-free-'));
try {
  const manifest = JSON.parse(await readFile(join(root, 'packages/core/package.json'), 'utf8'));
  const core = join(fixture, 'node_modules/@webdock/core');
  await mkdir(dirname(core), { recursive: true });
  await cp(join(root, 'packages/core'), core, { recursive: true });
  for (const dependency of Object.keys(manifest.dependencies)) {
    const target = join(fixture, 'node_modules', dependency);
    await mkdir(dirname(target), { recursive: true });
    await symlink(join(root, 'node_modules', dependency), target);
  }
  const imports = [];
  for (const [name, target] of Object.entries(manifest.exports)) {
    if (!name.includes('*')) imports.push('@webdock/core/' + name.slice(2));
    else {
      const directory = dirname(target);
      for (const file of await readdir(join(core, directory))) {
        if (file.endsWith('.ts') && !file.endsWith('.test.ts'))
          imports.push('@webdock/core/' + name.slice(2).replace('*', file.slice(0, -3)));
      }
    }
  }
  const entry = join(fixture, 'entry.ts');
  await writeFile(entry, imports.map((name, index) => `import * as e${index} from ${JSON.stringify(name)};`).join('\n') +
    `\nconsole.log(JSON.stringify([${imports.map((_, index) => `Object.keys(e${index})`).join(',')}]));\n`);
  const builder = join(fixture, 'build.ts');
  await writeFile(builder, `
const forbidden = /^@webdock\\/(?:module-(?:mail|hosting|databases|storage|cms)(?:\\/|$)|adapter-|mail-|hosting-|database-|cms(?:-|\\/|$)|payload-|page-builder(?:\\/|$))/;
const result = await Bun.build({entrypoints:[${JSON.stringify(entry)}], target:'bun', outdir:${JSON.stringify(join(fixture, 'dist'))}, plugins:[{name:'no-product-packages',setup(build){build.onResolve({filter:forbidden}, ({path, importer}) => { throw Error('Forbidden product import '+path+' from '+importer); });}}]});
if (!result.success) { for (const log of result.logs) console.error(log); process.exit(1); }
`);
  const build = spawnSync(bun, ['--no-env-file', builder], { cwd: fixture, encoding: 'utf8', timeout: 30000 });
  assert.equal(build.status, 0, build.stderr || build.stdout);
  const originalEntry = await readFile(entry, 'utf8');
  await writeFile(entry, "import '@webdock/module-mail/manifest';\n" + originalEntry);
  const denied = spawnSync(bun, ['--no-env-file', builder], { cwd: fixture, encoding: 'utf8', timeout: 30000 });
  assert.notEqual(denied.status, 0, 'Resolver must reject product package imports');
  assert.match(denied.stderr, /Forbidden product import @webdock\/module-mail\/manifest/);
  const run = spawnSync(bun, ['--no-env-file', join(fixture, 'dist/entry.js')], { cwd: fixture, encoding: 'utf8', timeout: 30000 });
  assert.equal(run.status, 0, run.stderr || run.stdout);
  assert.equal(JSON.parse(run.stdout.trim()).length, imports.length);
  console.log(`Built and imported ${imports.length} @webdock/core exports with all product package imports denied.`);
} finally {
  await rm(fixture, { recursive: true, force: true });
}
