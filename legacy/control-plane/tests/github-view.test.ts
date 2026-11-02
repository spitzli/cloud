import { expect, test } from "bun:test";

test("GitHub picker preserves large installation IDs while bounding pagination", () => {
  const result = Bun.spawnSync(
    [
      process.execPath,
      "--no-env-file",
      "-e",
      `
    import assert from "node:assert/strict";
    import { mock } from "bun:test";
    const calls = [];
    const installations = [{ id:7, account:"first-account" }, { id:12345678, account:"second-account" }];
    mock.module("./apps/control-plane/src/modules/integrations/operator", () => ({ currentRegistryActor: async () => ({user:{id:"operator"}}) }));
    mock.module("./apps/control-plane/src/modules/portal/request-context", () => ({ browserHeaders: () => new Headers({cookie:"fixture"}) }));
    mock.module("./apps/control-plane/src/modules/integrations/github/client", () => ({
      githubOrigin: () => "https://studio.example.test",
      getGitHub: () => ({
        session: async (_headers, actor) => { assert.equal(actor,"operator"); return {login:"operator-github"}; },
        installations: async (_session, page) => { calls.push(["installations",page]); return {items:installations,totalPages:10000}; },
        repositories: async (_session, installation, installationPage, page) => {
          calls.push(["repositories",installation,installationPage,page]);
          return {items:[{id:11,name:installation === 12345678 ? "second-account/private-plugin" : "first-account/other",url:"https://github.com/example/plugin",private:true}],totalPages:10000};
        },
        installURL:"https://github.com/apps/test/installations/new",
        permissionsURL:"https://github.com/settings/connections/applications/test",
        callbackURL:"https://studio.example.test/api/github/callback",
      }),
    }));
    const { githubView } = await import("./apps/control-plane/src/modules/integrations/github/view");
    const selected = await githubView({installation:"12345678",installationPage:"2",page:"3"});
    assert.equal(selected.installation,12345678);
    assert.equal(selected.repositories[0].name,"second-account/private-plugin");
    assert.deepEqual(calls,[ ["installations",2], ["repositories",12345678,2,3] ]);
    calls.length=0;
    const bounded = await githubView({installation:"12345678",installationPage:"10001",page:"9007199254740992"});
    assert.equal(bounded.installation,12345678);
    assert.deepEqual(calls,[ ["installations",1], ["repositories",12345678,1,1] ]);
    assert.equal((await githubView({installation:"12345678",installationPage:"10000",page:"10000"})).page,10000);
    for (const installation of ["9007199254740992","0","-1","1.5","Infinity"]) {
      assert.equal((await githubView({installation})).installation,7);
    }
  `,
    ],
    { cwd: new URL("../../../", import.meta.url).pathname },
  );
  expect(result.stderr.toString()).toBe("");
  expect(result.exitCode).toBe(0);
});
