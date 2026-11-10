import test from 'node:test';
import assert from 'node:assert/strict';
import { createHostingGitVercel } from '../src/server/git-vercel.ts';

const credential = { token: 'synthetic-publisher-token', teamID: 'team_fixture', configurationID: 'icfg_fixture', projectID: 'prj_fixture', generation: 1 };
const project = { id: 'prj_fixture', accountId: 'team_fixture', link: null, functionDefaultRegions: ['fra1'], functionZeroConfigFailover: false, framework: 'nextjs', nodeVersion: '24.x', buildCommand: 'npm run build', rootDirectory: null };
const configuration = { id: 'icfg_fixture', teamId: 'team_fixture', scopes: ['read:project', 'read-write:deployment', 'read:integration-configuration'], projects: ['prj_fixture'] };
const { prepareGitVercelBuild } = createHostingGitVercel({ env: {}, decryptMailSecret: () => { throw Error('Unused fixture dependency'); }, vercelJSON: () => { throw Error('Network is unavailable in this fixture'); } });
for (const key of ['SPITZLI_WORKER_TOKEN', 'WEBDOCK_WORKER_TOKEN']) {
  test(`customer builds refuse reserved namespace ${key} before provider reads`, async () => {
    let calls = 0;
    await assert.rejects(prepareGitVercelBuild(credential, { buildEnvironment: { [key]: 'fixture-value' }, allowedBuildVariables: [key] }, async path => {
      calls++;
      return path.includes('configuration') ? configuration : project;
    }));
    assert.equal(calls, 0);
  });
}
test('explicitly approved application variables retain their values without publisher credentials', async () => {
  const result = await prepareGitVercelBuild(credential, { buildEnvironment: { PUBLIC_ENDPOINT: 'https://example.test' }, allowedBuildVariables: ['PUBLIC_ENDPOINT'] }, async path => path.includes('configuration') ? configuration : project);
  assert.deepEqual(result.buildEnvironment, { PUBLIC_ENDPOINT: 'https://example.test' });
  assert.equal(JSON.stringify(result).includes(credential.token), false);
});
