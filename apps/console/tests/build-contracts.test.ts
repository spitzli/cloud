import assert from "node:assert/strict";
import { test } from "node:test";
import { buildSchema, buildCommandSchema, buildOutcome, canCancelBuild, emptyLogWindow, mergeLogPage, sourceSchema, sourceSettingsCommand } from "../src/build-contracts";

await test("build commands bound project IDs and explicit nonsecret strategy settings", () => {
  assert.throws(() => buildCommandSchema.parse({ action: "git.builds.cancel", projectID: "01", buildID: "201" }));
  assert.throws(() => buildCommandSchema.parse({ action: "git.builds.cancel", projectID: "101", buildID: "201", operator: true }));
  const source = sourceSchema.parse({ id: "301", projectID: "101", targetID: "201", connectionID: "401", repositoryID: "55", branch: "main", rootDirectory: ".", recipe: "dockerfile", revision: 2, enabled: true, autoPublish: false, executionBackend: "microvm", buildProvider: "isolated" });
  const command = sourceSettingsCommand(source, { executionBackend: "kubernetes", strategy: "railpack", contextDirectory: "web", dockerfile: "Dockerfile" });
  assert.equal(command.action, "git.source.configure");
  assert.equal(command.connectionID, "401");
  assert.equal(command.containerBuild?.strategy, "railpack");
  assert.equal("buildEnvironment" in command, false);
  for(const branch of ["", "-option", "a..b", "a//b", "a/", "topic.lock", "topic name"])
    assert.throws(()=>buildCommandSchema.parse({...command,branch}));
  assert.equal(buildCommandSchema.parse({...command,branch:"feature/my-change"}).action,"git.source.configure");
  assert.throws(() => sourceSettingsCommand(source, { executionBackend: "kubernetes", strategy: "dockerfile", contextDirectory: "../other", dockerfile: "Dockerfile" }));
});

await test("build success remains separate from failed rollout and cancellation requires current write authority", () => {
  assert.equal(buildOutcome("artifact-ready"), "Awaiting publication");
  assert.equal(buildOutcome("succeeded"), "Build succeeded");
  for (const status of ["queued", "running", "artifact-ready"] as const) {
    assert.equal(canCancelBuild(status, true, false), true);
    assert.equal(canCancelBuild(status, false, false), false);
    assert.equal(canCancelBuild(status, true, true), false);
  }
  assert.equal(canCancelBuild("succeeded", true, false), false);
});

await test("log cursor resumption rejects backward pages and keeps a bounded text window", () => {
  const first = mergeLogPage(emptyLogWindow(), { entries: [{ cursor: "9", generation: 1, phase: "building", text: "<script>owned</script>", createdAt: null }], cursor: "9", status: "running", phase: "building", terminal: false });
  const replay = mergeLogPage(first, { entries: [], cursor: "9", status: "running", phase: "building", terminal: false });
  assert.equal(replay.cursor, "9");
  assert.equal(replay.entries.length, 1);
  assert.throws(() => mergeLogPage(first, { entries: [], cursor: "8", status: "running", phase: "building", terminal: false }));
  let window = first;
  for (let n = 10; n < 30; n++) window = mergeLogPage(window, { entries: [{ cursor: String(n), generation: 1, phase: "building", text: "x".repeat(8192), createdAt: null }], cursor: String(n), status: "running", phase: "building", terminal: false });
  assert.equal(window.cursor, "29");
  assert.ok(window.entries.reduce((sum, entry) => sum + entry.text.length, 0) <= 65536);
  assert.equal(window.truncated, true);
});


await test("retry DTO fields preserve only bounded counts, dates and known infrastructure failures", () => {
  const base = {id:"501",projectID:"101",sourceID:"301",targetID:"201",sourceSHA:"a".repeat(40),status:"queued",revision:2,generation:1,createdAt:null,finishedAt:null,failureCode:null};
  const parsed = buildSchema.parse({...base,retryCount:2,nextEligibleAt:"2026-10-30T14:00:00Z",lastInfrastructureFailure:"KUBERNETES_NODE_UNAVAILABLE"});
  assert.equal(parsed.retryCount,2);
  assert.equal(parsed.nextEligibleAt,"2026-10-30T14:00:00Z");
  assert.equal(parsed.lastInfrastructureFailure,"KUBERNETES_NODE_UNAVAILABLE");
  assert.equal(buildSchema.parse(base).retryCount,0);
  for(const retryCount of [-1,3,0.5]) assert.equal(buildSchema.safeParse({...base,retryCount}).success,false);
  assert.equal(buildSchema.safeParse({...base,nextEligibleAt:"tomorrow"}).success,false);
  assert.equal(buildSchema.safeParse({...base,lastInfrastructureFailure:"https://private.invalid/token"}).success,false);
});

await test("quality workflow mode and exact path survive ordinary source settings saves",()=>{
  const source=sourceSchema.parse({id:"301",projectID:"101",targetID:"201",connectionID:"401",repositoryID:"55",branch:"release",rootDirectory:".",recipe:"dockerfile",revision:2,enabled:true,autoPublish:true,executionBackend:"kubernetes",buildProvider:"isolated",webhookTrigger:"workflow_run",workflowPath:".github/workflows/quality.yml"});
  const command=sourceSettingsCommand(source,{executionBackend:"kubernetes",strategy:"railpack",contextDirectory:".",dockerfile:"Dockerfile"});
  assert.equal(command.webhookTrigger,"workflow_run");assert.equal(command.workflowPath,".github/workflows/quality.yml");
  assert.throws(()=>sourceSettingsCommand(source,{executionBackend:"microvm",strategy:"auto",contextDirectory:".",dockerfile:"Dockerfile"}));
  const omitted={...command};delete omitted.webhookTrigger;
  assert.equal(buildCommandSchema.parse(omitted).action,"git.source.configure");
});
