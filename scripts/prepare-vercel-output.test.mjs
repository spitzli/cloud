import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, rm, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { prepareVercelOutput } from './prepare-vercel-output.mjs';

test('prebuilt output omits dotenv references without touching source secrets and pins Frankfurt', async () => {
  const root = await mkdtemp(join(tmpdir(), 'webdock-prebuilt-'));
  try {
    const fn = join(root, 'output/functions/app.func'); await mkdir(fn, { recursive: true });
    const source = join(root, '.env.private'); await writeFile(source, 'PRIVATE=fixture');
    const config = join(fn, '.vc-config.json');
    await writeFile(config, JSON.stringify({ environment: {}, runtime: 'nodejs24.x', filePathMap: { 'env': 'legacy/auth/.env.private', 'app.js': 'legacy/auth/app.js' } }));
    assert.equal((await prepareVercelOutput(join(root, 'output'))).removedEnvironmentReferences, 1);
    const result = JSON.parse(await readFile(config));
    assert.deepEqual(result.filePathMap, { 'app.js': 'legacy/auth/app.js' });
    assert.deepEqual(result.regions, ['fra1']);
    assert.equal(await readFile(source, 'utf8'), 'PRIVATE=fixture');
    await writeFile(config, JSON.stringify({ environment: { PRIVATE: 'unexpected' } }));
    await assert.rejects(prepareVercelOutput(join(root, 'output')), /environment overrides/);
  } finally { await rm(root, { recursive: true }); }
});

test('a parallel root slot cannot omit the actual home page and its traced files from a prebuilt function', async () => {
  const root = await mkdtemp(join(tmpdir(), 'webdock-prebuilt-root-'));
  try {
    const output = join(root, '.vercel/output'), fn = join(output, 'functions/shared.func');
    const server = join(root, 'legacy/console/.next/server'), page = 'app/(console)/(workspace)/page.js';
    await mkdir(fn, { recursive: true });
    await symlink('shared.func', join(output, 'functions/index.func'));
    await mkdir(join(server, 'app/(console)/(workspace)'), { recursive: true });
    await mkdir(join(server, 'chunks'), { recursive: true });
    await writeFile(join(server, 'app-paths-manifest.json'), JSON.stringify({ '/(console)/(workspace)/page': page, '/(console)/@toolbar/page': 'app/(console)/@toolbar/page.js' }));
    await writeFile(join(server, page), 'module.exports = {};');
    await writeFile(join(server, page + '.nft.json'), JSON.stringify({ files: ['../../../chunks/home.js'] }));
    await writeFile(join(server, 'chunks/home.js'), 'module.exports = {};');
    await writeFile(join(server, page.replace('.js', '_client-reference-manifest.js')), 'self.manifest = {};');
    const file = join(fn, '.vc-config.json');
    await writeFile(file, JSON.stringify({ handler: 'legacy/console/___next_launcher.cjs', environment: {}, filePathMap: {} }));
    await prepareVercelOutput(output);
    const config = JSON.parse(await readFile(file));
    for (const path of [page, 'chunks/home.js', page.replace('.js', '_client-reference-manifest.js')]) {
      assert.equal(config.filePathMap['legacy/console/.next/server/' + path], 'legacy/console/.next/server/' + path);
    }
  } finally { await rm(root, { recursive: true, force: true }); }
});
