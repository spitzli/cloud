import "./router-test-environment";
import assert from "node:assert/strict";
import { after, test } from "node:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { QueryClient, QueryClientProvider, notifyManager, defaultScheduler } from "@tanstack/react-query";
import { Window } from "happy-dom";
import { I18nProvider } from "@webdock/i18n/react";
import { ApiClient, type Session } from "../src/api";
import { ApiContext, SessionContext } from "../src/context";
import { HostingBuilds } from "../src/HostingBuilds";
import { buildCommandSchema, type Build, type BuildCommand, type BuildSource } from "../src/build-contracts";
import { readBuildLogs } from "../src/build-client";

notifyManager.setScheduler(queueMicrotask);
after(() => { notifyManager.setScheduler(defaultScheduler); });
const source: BuildSource = { id: "301", projectID: "101", targetID: "201", connectionID: "401", repositoryID: "55", branch: "main", rootDirectory: ".", recipe: "dockerfile", revision: 2, enabled: true, webhookEnabled:false, webhookTrigger:"push", autoPublish: false, executionBackend: "microvm", buildProvider: "isolated", healthPath: "/", workflowPath: ".github/workflows/webdock.yml", artifactPrefix: "webdock", containerBuild: { strategy: "auto", contextDirectory: ".", dockerfile: "Dockerfile", buildArgs: {} } };
const project = { projectID: "101", name: "Owned project", customerID: "11", customerName: "Owned customer", operator: false, configured: true, provider: "k3s" };
function build(id = "501", status: Build["status"] = "succeeded"): Build { return { id, projectID: "101", sourceID: "301", targetID: "201", sourceSHA: (id === "501" ? "a" : "b").repeat(40), status, revision: 2, generation: 1, retryCount:0, createdAt: "2026-10-29T12:00:00Z", finishedAt: status === "running" ? null : "2026-10-29T12:02:00Z", logs: "", failureCode: null, executionBackend: "kubernetes", phase: status === "running" ? "building" : status, registryVerifiedAt: status === "succeeded" ? "2026-10-29T12:02:00Z" : null }; }
const release = { id: "601", projectID: "101", sourceID: "301", targetID: "201", artifactID: "701", buildID: "501", status: "failed", revision: 1, approvedBy: "actor", operationID: null, createdAt: "2026-10-29T12:02:00Z" };
function button(host: ParentNode, label: string): HTMLButtonElement { const value = [...host.querySelectorAll<HTMLButtonElement>("button")].find((entry) => entry.textContent === label); assert.ok(value, `Missing button ${label}`); return value; }
async function flush(action: () => void = () => undefined) { await act(async () => { action(); await new Promise((resolve) => setTimeout(resolve, 0)); }); }
async function until(check: () => boolean) { for (let n = 0; n < 80; n++) { if (check()) return; await flush(); } assert.ok(check(), "Build UI did not settle"); }
function fixture(options: { write?: boolean; preview?: boolean; running?: boolean; noSource?:boolean; connectionState?:string; provider?:string; rejectFirstConfigure?:boolean; releaseStatus?: "awaiting-approval" | "ready" | "failed"; stream?:"resume"|"denied"|"terminal-backlog"|"polled-terminal";locale?:"en"|"de";buildData?:Partial<Build>;workflowGate?:boolean } = {}) {
  const browser = new Window({ url: "https://console.example/hosting/projects/101/builds" });
  Object.assign(globalThis, { scrollTo: browser.scrollTo.bind(browser), window: browser, self: browser, navigator: browser.navigator, document: browser.document, location: browser.location, HTMLElement: browser.HTMLElement, Event: browser.Event, FormData: browser.FormData, IS_REACT_ACT_ENVIRONMENT: true });
  const host = document.createElement("div"); document.body.append(host);
  const root = createRoot(host), client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  const calls: BuildCommand[] = [], cursors: string[] = [], streamCursors:string[]=[];
  let currentSource:BuildSource = options.workflowGate ? {...source,webhookTrigger:"workflow_run",workflowPath:".github/workflows/quality.yml",executionBackend:"kubernetes"} : source;
  let hasSource=options.noSource!==true;
  let rejectConfigure=options.rejectFirstConfigure===true;
  let cancelled = false;
  const responseBuild = (...parameters: Parameters<typeof build>) => ({...build(...parameters),...options.buildData});
  const api = new ApiClient({ apiOrigin: "https://console.example", authOrigin: "https://console.example", enabledFeatures: ["hosting"] }, (input, init) => {
    assert.ok(input instanceof URL); const url = input; assert.equal(url.origin, "https://console.example"); assert.equal(init?.credentials, "include");
    if (init.method !== "POST") {
      if(url.pathname.endsWith("/events")){
        streamCursors.push(new Headers(init.headers).get("Last-Event-ID") ?? "");
        if(options.stream==="denied")return Promise.resolve(Response.json({error:{message:"Access revoked"}},{status:403}));
        if(options.stream==="terminal-backlog"){
          const entries=Array.from({length:30},(_,i)=>({cursor:String(i+2),generation:1,phase:"building",text:`terminal-line-${String(i+2)}`,createdAt:null}));
          const frames=[entries.slice(0,25),entries.slice(25)].map(entries=>{const cursor=entries.at(-1)?.cursor;assert.ok(cursor);const frame={entries,cursor,status:"succeeded",phase:"succeeded",terminal:true};return `event: logs\nid: ${frame.cursor}\ndata: ${JSON.stringify(frame)}\n\n`;}).join("");
          return Promise.resolve(new Response(frames,{headers:{"Content-Type":"text/event-stream"}}));
        }
        const last=streamCursors.length>1;
        const frame={entries:[{cursor:last?"3":"2",generation:1,phase:"building",text:last?"terminal streamed output":"live streamed output",createdAt:null}],cursor:last?"3":"2",status:last?"succeeded":"running",phase:last?"succeeded":"building",terminal:last};
        return Promise.resolve(new Response(`event: logs\nid: ${frame.cursor}\ndata: ${JSON.stringify(frame)}\n\n`,{headers:{"Content-Type":"text/event-stream"}}));
      }

      const after = url.searchParams.get("after") ?? "0"; cursors.push(after);
      if(options.stream==="polled-terminal" && after!=="0")return Promise.resolve(Response.json({entries:[{cursor:"3",generation:1,phase:"succeeded",text:"retained final output",createdAt:null}],cursor:"3",status:"succeeded",phase:"succeeded",terminal:true}));
      return Promise.resolve(Response.json({ entries: [{ cursor: after === "0" ? "1" : "2", generation: 1, phase: "building", text: after === "0" ? "<script>owned output</script>" : "resumed output", createdAt: null }], cursor: after === "0" ? "1" : "2", status: options.stream ? "running":"succeeded", phase: options.stream ? "building":"succeeded", terminal: !options.stream }));
    }
    const raw: unknown = JSON.parse(typeof init.body === "string" ? init.body : "{}");
    if (typeof raw === "object" && raw !== null && "action" in raw && raw.action === "projects.get") return Promise.resolve(Response.json({...project,provider:options.provider ?? project.provider}));
    const command = buildCommandSchema.parse(raw); calls.push(command);
    switch (command.action) {
      case "git.connections.list": return Promise.resolve(Response.json({docs:[{id:"401",customerID:"11",accountLogin:"owned-account",state:options.connectionState ?? "active"}]}));
      case "git.repositories.list": return Promise.resolve(Response.json({docs:[{repositoryID:"55",fullName:"owned/project",defaultBranch:"main"}]}));
      case "git.sources.list": return Promise.resolve(Response.json({ docs: hasSource ? [currentSource] : [] }));
      case "git.source.get": return Promise.resolve(Response.json({ source: hasSource ? currentSource : null, targets: [{ id: "201", name: "Website" }], blockers: [], canBuild: options.write === true, canConfigure: options.write === true, canApprove: options.write === true }));
      case "git.builds.list": return Promise.resolve(Response.json({ docs: [responseBuild(command.page === 1 ? "501" : "502", cancelled ? "cancelled" : options.running || command.page === 2 ? "running" : "succeeded")], page: command.page, limit: 20, totalPages: 2 }));
      case "git.builds.get": return Promise.resolve(Response.json(responseBuild(command.buildID, cancelled ? "cancelled" : options.running || command.buildID === "502" ? "running" : "succeeded")));
      case "git.releases.list": return Promise.resolve(Response.json({ docs: [{...release,status:options.releaseStatus ?? "failed"}], page: command.page, limit: 20, totalPages: 1 }));
      case "git.builds.cancel": cancelled = true; return Promise.resolve(Response.json(responseBuild(command.buildID, "cancelled")));
      case "git.builds.request": return Promise.resolve(Response.json(responseBuild("502", "queued")));
      case "git.source.webhook.configure": currentSource={...currentSource,webhookEnabled:command.enabled};return Promise.resolve(Response.json({sourceID:currentSource.id,revision:currentSource.revision,webhookEnabled:command.enabled}));
      case "git.source.configure": if(rejectConfigure){rejectConfigure=false;return Promise.resolve(Response.json({error:{message:"Source changed"}},{status:409}));}hasSource=true;currentSource = { ...currentSource, revision: command.revision + 1, targetID:command.targetID,connectionID:command.connectionID,repositoryID:command.repositoryID,branch:command.branch,rootDirectory:command.rootDirectory,webhookEnabled:false, executionBackend: command.executionBackend, containerBuild: command.containerBuild ?? currentSource.containerBuild }; return Promise.resolve(Response.json(currentSource));
      case "git.releases.approve": case "git.releases.rollback": return Promise.resolve(Response.json(release));
    }
  });
  const session: Session = { user: { id: "actor", name: "Owned owner", email: "owned@example.invalid", role: "user", emailVerified: true, preview: options.preview === true }, session: { id: "owned-session", userId: "actor", expiresAt: "2099-01-01T00:00:00Z" } };
  return { host, client, calls, cursors, streamCursors, render: async () => {
    const parent = createRootRoute({ component: () => <ApiContext.Provider value={api}><SessionContext.Provider value={session}><I18nProvider locale={options.locale ?? "en"} preference={options.locale ?? "en"}><Outlet /></I18nProvider></SessionContext.Provider></ApiContext.Provider> });
    const route = createRoute({ getParentRoute: () => parent, path: "/hosting/projects/$projectID/builds", component: () => <HostingBuilds projectID="101" /> });
    const router = createRouter({ routeTree: parent.addChildren([route]), history: createMemoryHistory({ initialEntries: ["/hosting/projects/101/builds"] }), isServer: false });
    await router.load(); await flush(() => { root.render(<QueryClientProvider client={client}><RouterProvider router={router} /></QueryClientProvider>); });
  }, close: async () => { await flush(() => { root.unmount(); }); client.clear(); await browser.happyDOM.abort(); } };
}

await test("rendered build success and rollout failure remain distinct; reads resume and pagination never grants cancellation", async () => {
  const view = fixture();
  try {
    await view.render(); await until(() => (view.host.textContent.includes("Rollout failed")) && view.host.querySelector("pre") !== null);
    assert.ok(view.host.textContent.includes("Build succeeded"));
    assert.equal(view.host.querySelector("script"), null); assert.ok(view.host.querySelector("pre")?.textContent.includes("<script>owned output</script>"));
    assert.equal([...view.host.querySelectorAll("button")].some((entry) => entry.textContent === "Cancel build"), false);
    await flush(() => { button(view.host, "Refresh logs").click(); }); await until(() => view.cursors.includes("1"));
    assert.ok(view.host.querySelector("pre")?.textContent.includes("resumed output"));
    const history = view.host.querySelector('[aria-label="Build history"]'); assert.ok(history);
    await flush(() => { button(history, "Next").click(); }); await until(() => view.calls.some((command) => command.action === "git.builds.list" && command.page === 2));
    await until(() => view.host.textContent.includes("Build bbbbbbbb"));
    assert.equal([...view.host.querySelectorAll("button")].some((entry) => entry.textContent === "Cancel build"), false);
    assert.equal(view.calls.some((command) => command.action === "git.builds.cancel"), false);
  } finally { await view.close(); }
});

await test("current native capability permits cancellation while customer preview remains read-only", async () => {
  const preview = fixture({ write: true, preview: true, running: true });
  try { await preview.render(); await until(() => preview.host.querySelector("pre") !== null); assert.equal([...preview.host.querySelectorAll("button")].some((entry) => ["Cancel build", "Build latest commit", "Save build settings"].includes(entry.textContent)), false); } finally { await preview.close(); }
  const write = fixture({ write: true, running: true });
  try { await write.render(); await until(() => write.host.textContent.includes("Cancel build")); await flush(() => { button(write.host, "Cancel build").click(); }); await until(() => write.calls.some((command) => command.action === "git.builds.cancel")); assert.deepEqual(write.calls.find((command) => command.action === "git.builds.cancel"), { action: "git.builds.cancel", projectID: "101", buildID: "501" }); } finally { await write.close(); }
});

await test("log history uses ApiClient credentials and exact validated cursor paths", async () => {
  const paths: string[] = [];
  const api = new ApiClient({ apiOrigin: "https://api.example", authOrigin: "https://auth.example", enabledFeatures: ["hosting"] }, (input, init) => { assert.ok(input instanceof URL); const url = input; paths.push(url.href); assert.equal(init?.credentials, "include"); assert.equal(init.redirect, "error"); return Promise.resolve(Response.json({ entries: [], cursor: "9", status: "running", phase: "building", terminal: false })); });
  await readBuildLogs(api, "101", "501", "9");
  assert.deepEqual(paths, ["https://api.example/api/console/hosting/projects/101/builds/501/logs?after=9"]);
  assert.throws(() => readBuildLogs(api, "../other", "501", "9"));
  assert.throws(() => readBuildLogs(api, "101", "501", "01"));
});

await test("manual requests and strategy saves keep the selected source binding", async () => {
  const view=fixture({write:true});
  try {
    await view.render();await until(()=>view.host.textContent.includes("Build latest commit"));
    await flush(()=>{button(view.host,"Build latest commit").click();});
    await until(()=>view.calls.some((command)=>command.action==="git.builds.request"));
    const requested=view.calls.find((command)=>command.action==="git.builds.request");assert.ok(requested);
    assert.equal(requested.targetID,"201");assert.match(requested.idempotencyKey,/^[A-Za-z0-9_-]{16,128}$/);
    const details=view.host.querySelector("details");assert.ok(details);details.open=true;
    const push=details.querySelector<HTMLInputElement>('input[type="checkbox"]');assert.ok(push);
    await flush(()=>{push.click();});await until(()=>view.calls.some((command)=>command.action==="git.source.webhook.configure"));
    await until(()=>push.checked);
    const execution=details.querySelector("select");assert.ok(execution);
    await flush(()=>{execution.value="kubernetes";execution.dispatchEvent(new Event("change",{bubbles:true}));});
    const strategy=details.querySelectorAll("select")[1];assert.ok(strategy);
    await flush(()=>{strategy.value="railpack";strategy.dispatchEvent(new Event("change",{bubbles:true}));});
    const form=details.querySelector("form");assert.ok(form);
    await flush(()=>{form.dispatchEvent(new Event("submit",{bubbles:true,cancelable:true}));});
    await until(()=>view.calls.some((command)=>command.action==="git.source.configure"));
    const configured=view.calls.find((command)=>command.action==="git.source.configure");assert.ok(configured);
    assert.equal(configured.executionBackend,"kubernetes");assert.equal(configured.containerBuild?.strategy,"railpack");assert.equal(configured.connectionID,"401");assert.equal(configured.repositoryID,"55");assert.equal(configured.branch,"main");assert.equal("buildEnvironment" in configured,false);
  } finally {await view.close();}
});

await test("a project manager can connect the first already-authorized repository",async()=>{
  const view=fixture({write:true,noSource:true});
  try {await view.render();await until(()=>view.host.textContent.includes("owned/project"));const form=view.host.querySelector<HTMLFormElement>('form[aria-label="Connect a repository"]');assert.ok(form);await flush(()=>{form.dispatchEvent(new Event("submit",{bubbles:true,cancelable:true}));form.dispatchEvent(new Event("submit",{bubbles:true,cancelable:true}));});await until(()=>view.calls.some(c=>c.action==="git.source.configure"));assert.equal(view.calls.filter(c=>c.action==="git.source.configure").length,1);const command=view.calls.find(c=>c.action==="git.source.configure");assert.ok(command);assert.equal(command.connectionID,"401");assert.equal(command.repositoryID,"55");assert.equal(command.targetID,"201");assert.equal(command.revision,0);assert.equal(command.executionBackend,"kubernetes");assert.equal(command.autoPublish,false);assert.equal("buildEnvironment" in command,false);assert.ok(view.calls.some(c=>c.action==="git.connections.list" && c.customerID==="11"));} finally {await view.close();}
});

await test("source setup stays hidden for viewers and read-only customer preview",async()=>{
  for(const options of [{write:false},{write:true,preview:true}]){const view=fixture({...options,noSource:true});try {await view.render();await until(()=>view.host.textContent.includes("No repository source"));assert.equal(view.host.querySelector('form[aria-label="Connect a repository"]'),null);assert.equal(view.calls.some(c=>c.action==="git.connections.list"),false);} finally {await view.close();}}
});

await test("disconnected GitHub accounts explain the provider prerequisite without accepting a source",async()=>{
  const view=fixture({write:true,noSource:true,connectionState:"disconnected"});
  try {await view.render();await until(()=>view.host.textContent.includes("No active GitHub connection"));assert.equal(view.host.querySelector('form[aria-label="Connect a repository"]'),null);assert.equal(view.calls.some(c=>c.action==="git.repositories.list" || c.action==="git.source.configure"),false);} finally {await view.close();}
});

await test("a fast rejected source save can be submitted again with the production query scheduler",async()=>{
  notifyManager.setScheduler(defaultScheduler);
  const view=fixture({write:true,noSource:true,rejectFirstConfigure:true});
  try {
    await view.render();await until(()=>view.host.textContent.includes("owned/project"));
    const first=view.host.querySelector<HTMLFormElement>('form[aria-label="Connect a repository"]');assert.ok(first);
    await flush(()=>first.dispatchEvent(new Event("submit",{bubbles:true,cancelable:true})));
    await until(()=>view.host.textContent.includes("Source changed"));
    const retry=view.host.querySelector<HTMLFormElement>('form[aria-label="Connect a repository"]');assert.ok(retry);
    await flush(()=>retry.dispatchEvent(new Event("submit",{bubbles:true,cancelable:true})));
    await until(()=>view.calls.filter(c=>c.action==="git.source.configure").length===2);
  } finally {await view.close();notifyManager.setScheduler(queueMicrotask);}
});

await test("an authorization refresh requires an explicit valid repository selection",async()=>{
  const view=fixture({write:true,noSource:true});
  try {await view.render();await until(()=>view.host.textContent.includes("owned/project"));const form=view.host.querySelector<HTMLFormElement>('form[aria-label="Connect a repository"]');assert.ok(form);const label=[...form.querySelectorAll("label")].find(value=>value.textContent.startsWith("Repository"));const select=label?.querySelector("select");assert.ok(select);await flush(()=>select.dispatchEvent(new Event("change",{bubbles:true})));await flush(()=>{view.client.setQueryData(["git-repositories","11","401"],{docs:[{repositoryID:"56",fullName:"owned/other",defaultBranch:"main"}]});});assert.equal(select.value,"");assert.equal(button(form,"Connect repository").disabled,true);await flush(()=>{select.value="56";select.dispatchEvent(new Event("change",{bubbles:true}));});await flush(()=>form.dispatchEvent(new Event("submit",{bubbles:true,cancelable:true})));await until(()=>view.calls.some(c=>c.action==="git.source.configure"));const command=view.calls.find(c=>c.action==="git.source.configure");assert.ok(command);assert.equal(command.repositoryID,"56");} finally {await view.close();}
});

await test("unsupported Vercel projects do not offer a source form or fetch Git account inventory",async()=>{
  const view=fixture({write:true,noSource:true,provider:"vercel"});
  try {await view.render();await until(()=>view.host.textContent.includes("Repository setup is unavailable"));assert.equal(view.host.querySelector('form[aria-label="Connect a repository"]'),null);assert.equal(view.calls.some(c=>c.action==="git.connections.list"),false);} finally {await view.close();}
});

await test("publication approval and redeployment use their own current revisions", async()=>{
  for(const [status,label,action] of [["awaiting-approval","Approve publication","git.releases.approve"],["ready","Redeploy this version","git.releases.rollback"]] as const){
    const view=fixture({write:true,releaseStatus:status});
    try {await view.render();await until(()=>view.host.textContent.includes(label));await flush(()=>{button(view.host,label).click();});await until(()=>view.calls.some((command)=>command.action===action));const command=view.calls.find((entry)=>entry.action===action);assert.ok(command);assert.ok(command.action==="git.releases.approve" || command.action==="git.releases.rollback");assert.equal(command.releaseID,"601");assert.equal(command.revision,1);} finally {await view.close();}
  }
});

await test("active SSE resumes from retained history and reconnects at the last acknowledged event",async()=>{
  const view=fixture({running:true,stream:"resume"});
  try {await view.render();await until(()=>view.host.textContent.includes("live streamed output"));for(let n=0;n<120 && view.streamCursors.length<2;n++)await act(async()=>{await new Promise((resolve)=>setTimeout(resolve,15));});await until(()=>view.host.textContent.includes("terminal streamed output"));assert.deepEqual(view.streamCursors,["1","2"]);assert.equal(view.host.querySelector("script"),null);} finally {await view.close();}
});

await test("terminal SSE drains every retained page before stopping",async()=>{
  const view=fixture({running:true,stream:"terminal-backlog"});
  try {await view.render();await until(()=>view.host.textContent.includes("terminal-line-26"));await flush();assert.ok(view.host.querySelector("pre")?.textContent.includes("terminal-line-31"));assert.equal(view.streamCursors.length,1);} finally {await view.close();}
});

await test("a terminal status poll retrieves output retained after the last stream event",async()=>{
  const view=fixture({running:true,stream:"polled-terminal"});
  try {await view.render();await until(()=>view.host.textContent.includes("live streamed output"));await flush(()=>{view.client.setQueryData(["git-builds","101","detail","501"],build("501","succeeded"));});await until(()=>view.host.textContent.includes("retained final output"));assert.ok(view.cursors.includes("2"));} finally {await view.close();}
});

await test("revoked SSE authority stops streaming and clears retained log bytes",async()=>{
  const view=fixture({running:true,stream:"denied"});
  try {await view.render();await until(()=>view.host.textContent.includes("Unable to load"));assert.equal(view.host.querySelector("pre"),null);assert.equal(view.host.textContent.includes("owned output"),false);const cached=view.client.getQueryData<{window:{entries:unknown[]};page:{entries:unknown[]}}>(["git-builds","101","logs","501"]);assert.ok(cached);assert.deepEqual(cached.window.entries,[]);assert.deepEqual(cached.page.entries,[]);assert.equal(view.streamCursors.length,1);} finally {await view.close();}
});

await test("German build chrome preserves customer names, log output and native action values",async()=>{
  const view=fixture({write:true,locale:"de"});
  try {
    await view.render();await until(()=>view.host.textContent.includes("Build erfolgreich") && view.host.querySelector("pre")!==null);
    assert.ok(view.host.textContent.includes("Builds für Owned project"));
    assert.ok(view.host.textContent.includes("Bereitstellung fehlgeschlagen"));
    assert.ok(view.host.textContent.includes("Repository-Quelle"));
    assert.ok(view.host.textContent.includes("Builds bei GitHub-Pushes starten"));
    assert.ok(view.host.querySelector("pre")?.textContent.includes("<script>owned output</script>"));
    const push=view.host.querySelector<HTMLInputElement>('details input[type="checkbox"]');assert.ok(push);
    await flush(()=>{push.click();});await until(()=>view.calls.some((command)=>command.action==="git.source.webhook.configure"));
    assert.deepEqual(view.calls.find((command)=>command.action==="git.source.webhook.configure"),{action:"git.source.webhook.configure",projectID:"101",sourceID:"301",revision:2,enabled:true});
  } finally {await view.close();}
});


function detailValue(host: ParentNode, label: string) {
  const term=[...host.querySelectorAll("dt")].find((entry)=>entry.textContent===label);
  assert.ok(term,`Missing build detail ${label}`);
  return term.nextElementSibling?.textContent;
}
await test("retry details expose bounded attempts and earliest eligibility without claiming execution duration",async()=>{
  const view=fixture({buildData:{status:"queued",generation:2,retryCount:1,nextEligibleAt:"2099-10-30T14:00:00Z",lastInfrastructureFailure:"KUBERNETES_NODE_UNAVAILABLE",finishedAt:null}});
  try {
    await view.render();await until(()=>view.host.querySelector("pre")!==null);
    assert.equal(detailValue(view.host,"Attempt"),"2");assert.equal(detailValue(view.host,"Retries"),"1 of 2");
    assert.ok(view.host.textContent.includes("Eligible for retry after"));
    assert.ok(view.host.textContent.includes("The build node is temporarily unavailable."));
    assert.equal(view.host.textContent.includes("KUBERNETES_NODE_UNAVAILABLE"),false);
    assert.equal([...view.host.querySelectorAll("dt")].some((entry)=>entry.textContent==="Request to completion"),false);
    assert.ok(view.host.textContent.includes("Rollout failed"));
  } finally {await view.close();}
});
await test("completed build timing includes queue time and previous retries do not become rollout failure",async()=>{
  const view=fixture({buildData:{generation:3,retryCount:2,lastInfrastructureFailure:"KUBERNETES_API_UNAVAILABLE"}});
  try {
    await view.render();await until(()=>view.host.querySelector("pre")!==null);
    assert.equal(detailValue(view.host,"Request to completion"),"120 s");
    assert.equal(detailValue(view.host,"Retries"),"2 of 2");
    assert.ok(view.host.textContent.includes("Build succeeded"));assert.ok(view.host.textContent.includes("Rollout failed"));
    const selected=view.host.querySelector('[aria-label="Selected build"]');assert.ok(selected);
    assert.equal(selected.querySelector('[role="alert"]'),null);
    assert.equal(selected.textContent.includes("Eligible for retry after"),false);
  } finally {await view.close();}
});
await test("safe build failure codes have useful messages without exposing raw codes",async()=>{
  for(const [failureCode,message] of [
    ["BUILD_DEADLINE_EXCEEDED","The build exceeded its time limit."],
    ["BUILD_MEMORY_LIMIT","The build exceeded its memory limit."],
    ["BUILD_RESOURCE_EVICTED","The build environment ran out of resources."],
    ["KUBERNETES_NODE_UNAVAILABLE","The build node is temporarily unavailable."],
    ["KUBERNETES_API_UNAVAILABLE","The build service is temporarily unavailable."],
    ["BUILD_FAILED","The build failed. Review the retained logs."],
  ] as const) {
    const view=fixture({buildData:{status:"failed",failureCode}});
    try {await view.render();await until(()=>view.host.querySelector("pre")!==null);const selected=view.host.querySelector('[aria-label="Selected build"]');assert.ok(selected);assert.equal(selected.querySelector('[role="alert"]')?.textContent,message);assert.equal(selected.textContent.includes(failureCode),false);} finally {await view.close();}
  }
});
await test("German retry metadata and safe resource failures are translated",async()=>{
  const view=fixture({locale:"de",buildData:{status:"queued",generation:2,retryCount:1,nextEligibleAt:"2099-10-30T14:00:00Z",lastInfrastructureFailure:"KUBERNETES_API_UNAVAILABLE",finishedAt:null}});
  try {
    await view.render();await until(()=>view.host.querySelector("pre")!==null);
    assert.equal(detailValue(view.host,"Versuch"),"2");assert.equal(detailValue(view.host,"Wiederholungen"),"1 von 2");
    assert.ok(view.host.textContent.includes("Erneuter Versuch frühestens ab"));
    assert.ok(view.host.textContent.includes("Der Build-Dienst ist vorübergehend nicht erreichbar."));
    assert.ok(view.host.querySelector("pre")?.textContent.includes("<script>owned output</script>"));
  } finally {await view.close();}
  const failure=fixture({locale:"de",buildData:{status:"failed",failureCode:"BUILD_MEMORY_LIMIT"}});
  try {await failure.render();await until(()=>failure.host.querySelector("pre")!==null);const selected=failure.host.querySelector('[aria-label="Ausgewählter Build"]');assert.ok(selected);assert.equal(selected.querySelector('[role="alert"]')?.textContent,"Der Build hat sein Arbeitsspeicherlimit überschritten.");} finally {await failure.close();}
});

await test("first queued build has no claimed execution or scheduled retry time",async()=>{
  const view=fixture({buildData:{status:"queued",generation:0,retryCount:0,nextEligibleAt:"2099-10-30T14:00:00Z",createdAt:null,finishedAt:null}});
  try {
    await view.render();await until(()=>view.host.querySelector("pre")!==null);
    assert.equal(detailValue(view.host,"Attempt"),"Not started");
    assert.equal(detailValue(view.host,"Requested"),"Not recorded");
    assert.equal(view.host.textContent.includes("Eligible for retry after"),false);
    assert.equal([...view.host.querySelectorAll("dt")].some((entry)=>entry.textContent==="Request to completion"),false);
  } finally {await view.close();}
});

await test("workflow-gated sources keep their trigger and exact workflow during settings edits",async()=>{
  const view=fixture({write:true,workflowGate:true});
  try {
    await view.render();await until(()=>view.host.textContent.includes("Build after GitHub workflow succeeds"));
    assert.ok(view.host.textContent.includes(".github/workflows/quality.yml"));
    const legacy=view.host.querySelector<HTMLOptionElement>('option[value="microvm"]');assert.ok(legacy);assert.equal(legacy.disabled,true);
    const form=view.host.querySelector("details form");assert.ok(form);
    await flush(()=>{form.dispatchEvent(new Event("submit",{bubbles:true,cancelable:true}));});
    await until(()=>view.calls.some((command)=>command.action==="git.source.configure"));
    const command=view.calls.find((entry)=>entry.action==="git.source.configure");assert.ok(command);
    assert.equal(command.webhookTrigger,"workflow_run");assert.equal(command.workflowPath,".github/workflows/quality.yml");
  } finally {await view.close();}
});
