import assert from "node:assert/strict";
import React from "react";
// The subset of Bun's runtime plugin API these fixtures use; Bun's global types stay out of the app build.
type Source = { contents: string; loader: "js" };
declare const Bun: {
  plugin(plugin: { name: string; setup(build: { module(name: string, load: () => Source): void; onLoad(options: { filter: RegExp }, load: () => Source): void }): void }): void;
  resolveSync(specifier: string, from: string): string;
};

// Exercise React's real server cache; the renderer normally supplies this request scope.
const internals = (React as unknown as {
  __SERVER_INTERNALS_DO_NOT_USE_OR_WARN_USERS_THEY_CANNOT_UPGRADE: {
    A: { getCacheForType: (factory: () => unknown) => unknown; getOwner: () => null } | null;
  };
}).__SERVER_INTERNALS_DO_NOT_USE_OR_WARN_USERS_THEY_CANNOT_UPGRADE;
function newRequest() {
  const values = new Map<() => unknown, unknown>();
  internals.A = { getOwner: () => null, getCacheForType(factory) {
    if (!values.has(factory)) values.set(factory, factory());
    return values.get(factory);
  } };
}
// Stand-ins for the Next.js request scope; navigation uses Next's real server build.
const stubs: Record<string, string> = {
  "next/navigation": `export * from ${JSON.stringify(Bun.resolveSync("next/dist/client/components/navigation.react-server.js", new URL(".", import.meta.url).pathname))};`,
  "next/link": "export default 'a'",
  "server-only": "export {};",
  "next/headers": 'export async function headers() { return new Headers({cookie: "webdock-sso=fixture"}); }',
  "@webdock/i18n/next": "export async function getRequestI18n() { return { t: text => text }; }",
};
Bun.plugin({
  name: "console-fixture-modules",
  setup(build) {
    for (const [name, contents] of Object.entries(stubs))
      build.module(name, () => ({ contents, loader: "js" }));
    // The hosting overview's client refresh button has no place in a server render.
    build.onLoad({ filter: /\/hosting\/src\/ui\/refresh\.tsx$/ }, () => ({
      contents: "export function HostingRefresh() { return null; }",
      loader: "js",
    }));
  },
});
process.env.WEBDOCK_API_ORIGIN = "https://api.example.test";
const calls: string[] = [];
let handle: (operation: string, args: unknown[]) => Promise<unknown>;
globalThis.fetch = async (_url, init) => {
  assert.equal(init?.cache, "no-store");
  const { operation, args } = JSON.parse(String(init?.body));
  calls.push(operation === "hosting.command" ? args[0].action : operation);
  const result = await handle(operation, args);
  return result instanceof Response ? result : Response.json({ data: result });
};
const { getConsoleContext } = await import("../../src/lib/console-context");
const { getManagedDatabasePage } = await import("../../src/lib/managed-database-page");
const { getHostingProject } = await import("../../src/lib/hosting-project");
const { registryClient } = await import("../../src/lib/registry-client");
const { studioCall } = await import("../../src/lib/studio-client");
const session = { user: { id: "1" }, operator: true, installation: { revision: "test", modules: { hosting: "enabled", databases: "enabled", mail: "enabled", cms: "enabled" } } };
const project = { projectID: "7", customerID: "8", name: "Project" };
const tenant = { id: "8", name: "Tenant", status: "active", role: "member" };
const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
};
const tick = () => new Promise(resolve => setImmediate(resolve));

newRequest();
handle = async () => { throw Error("Public access pages must not request workspace data"); };
for (const route of ["login", "waitlist", "waitlist/confirmed", "sign-up"]) {
  assert.equal(await getConsoleContext(route), null);
}

newRequest();
const projectGate = deferred<unknown>();
const listGate = deferred<unknown>();
handle = async operation => {
  if (operation === "session") return session;
  if (operation === "hosting.command") return projectGate.promise;
  if (operation === "listTenants") return listGate.promise;
  if (operation === "getTenant") return tenant;
  throw Error(operation);
};
const pendingContext = getConsoleContext("hosting/projects/7/overview");
await tick();
assert(calls.includes("projects.get"));
assert(calls.includes("listTenants"), "tenant list must start before the project read finishes");
projectGate.resolve(project);
await tick();
assert(!calls.includes("getTenant"), "navigation must not fetch members, invitations or site claims");
listGate.resolve([tenant]);
const context = await pendingContext;
assert.equal(context?.customerID, "8");
assert.deepEqual(context?.tenant, { tenant, canManage: true });
assert.equal(calls[0], "session");
assert.deepEqual(calls.slice(1).sort(), ["listTenants", "projects.get"]);

// Live capability removal wins over an unchanged admin label; grants do not require an admin label.
for (const [role, manageBilling, preview] of [["admin", false, false], ["member", true, false], ["member", true, true]] as const) {
  newRequest();
  handle = async operation => {
    if (operation === "session") return {...session, operator: false, preview};
    if (operation === "listTenants") return [{...tenant, role, capabilities: {manageProjects: false, createDeployments: false, manageDatabases: false, manageBilling}}];
    throw Error(operation);
  };
  const result = await getConsoleContext("tenants/8/overview");
  assert.equal(result?.tenant?.canManage, role === "admin" && !preview, "team role policy remains separate");
}

newRequest();
calls.length = 0;
handle = async () => ({ id: "7" });
await Promise.all([
  registryClient.findByID({ collection: "projects", id: "7", depth: 0 }),
  registryClient.findByID({ collection: "projects", id: "7", depth: 0 }),
]);
assert.equal(calls.length, 1, "equal registry reads should share one POST per render");
await registryClient.findByID({ collection: "projects", id: "9", depth: 0 });
assert.equal(calls.length, 2, "different resource IDs cannot share a result");
newRequest();
await registryClient.findByID({ collection: "projects", id: "7", depth: 0 });
assert.equal(calls.length, 3, "a new render must reauthorize and reload the same read");
await studioCall("manageTenant", ["8", { name: "A" }]);
await studioCall("manageTenant", ["8", { name: "A" }]);
assert.equal(calls.filter(call => call === "manageTenant").length, 2, "mutations must never be cached");
// Database navigation must load project context alongside permissions, not after them.
newRequest();
calls.length = 0;
const permissionGate = deferred<unknown>();
handle = async (operation, args) => {
  if (operation === "session") return session;
  if (operation === "listTenants") return [tenant];
  const command = args[0] as { action: string };
  if (command.action === "databases.get") return { id: "11", projectID: "7", name: "Database" };
  if (command.action === "databases.permissions") return permissionGate.promise;
  if (command.action === "projects.get") return project;
  throw Error(operation);
};
const databaseContext = getConsoleContext("hosting/databases/11/viewer");
await tick();
assert(calls.includes("databases.permissions"));
assert(calls.includes("projects.get"), "project context must not wait for database permissions");
permissionGate.resolve({ canWrite: false, operator: true });
const databaseNavigation = await databaseContext;
assert.equal(databaseNavigation?.projectID, "7");
assert.equal(databaseNavigation?.links.some(link => link.href.endsWith("/viewer")), false);
assert.deepEqual((await getManagedDatabasePage("11")).project, project);
assert.deepEqual(await getHostingProject("7"), project);
assert.equal(calls.filter(call => call === "projects.get").length, 1, "layout and navigation reuse the same project read");
assert.equal(calls.includes("getTenant"), false);

for (const failure of ["databases.permissions", "projects.get"]) {
  newRequest();
  handle = async (_operation, args) => {
    if (_operation === "session") return session;
    const { action } = args[0] as { action: string };
    if (action === "databases.get") return { id: "11", projectID: "7", name: "Database" };
    if (action === failure) return Response.json({ error: { kind: "HostingError", message: "Forbidden" } }, { status: 403 });
    return action === "projects.get" ? project : { canWrite: true, operator: true };
  };
  await assert.rejects(getManagedDatabasePage("11"), failure + " denial must prevent a database page result");
}

// Credential changes are visible on the next request, including a revoked session.
newRequest();
calls.length = 0;
handle = async operation => {
  assert.equal(operation, "session");
  return Response.json({ error: { kind: "PortalSessionMissing" } }, { status: 401 });
};
assert.equal(await getConsoleContext("hosting/projects/7/overview"), null);
assert.deepEqual(calls, ["session"]);

newRequest();
calls.length = 0;
handle = async operation => {
  assert.equal(operation, "session");
  return { ...session, preview: { status: "expired" } };
};
assert.deepEqual((await getConsoleContext("hosting/projects/7/overview"))?.tenants, []);
assert.deepEqual(calls, ["session"]);

// A single customer is still selected after their list becomes available.
newRequest();
handle = async operation => {
  if (operation === "session") return { ...session, operator: false };
  if (operation === "listTenants") return [tenant];
  if (operation === "getTenant") return tenant;
  throw Error(operation);
};
assert.equal((await getConsoleContext(""))?.customerID, "8");

// Navigation uses the same active/role policy without exposing preview management.
for (const [operator, role, status, preview, expected] of [
  [false, "member", "active", false, false],
  [false, "admin", "active", false, true],
  [false, "owner", "active", false, true],
  [false, "admin", "archived", false, false],
  [true, null, "active", false, true],
  [true, null, "archived", false, false],
  [false, "admin", "active", true, false],
] as const) {
  newRequest();
  calls.length = 0;
  handle = async operation => {
    if (operation === "session") return { ...session, operator, ...(preview ? { preview: { status: "active", customerID: "8" } } : {}) };
    if (operation === "listTenants") return [{ ...tenant, role, status }];
    throw Error(operation);
  };
  const navigation = await getConsoleContext("tenants/8/mail");
  assert.equal(navigation?.tenant?.canManage, expected, JSON.stringify({ operator, role, status, preview }));
  assert.equal(navigation?.tenant?.tenant.name, "Tenant");
  assert.deepEqual(calls, ["session", "listTenants"]);
}
newRequest();
handle = async operation => {
  if (operation === "session") return session;
  if (operation === "listTenants") return [{ ...tenant, id: "9" }];
  throw Error(operation);
};
const foreign = await getConsoleContext("tenants/8/mail");
assert.equal(foreign?.tenant, null);
assert.equal(foreign?.customerID, undefined);

// Forbidden tenant data stays hidden without masking a genuine outage.
newRequest();
handle = async operation => {
  if (operation === "session") return session;
  if (operation === "hosting.command") return project;
  return Response.json({ error: { kind: "AccessError", message: "Forbidden" } }, { status: 403 });
};
const denied = await getConsoleContext("hosting/projects/7/overview");
assert.equal(denied?.customerID, undefined);
assert.equal(denied?.tenant, null);
assert.deepEqual(denied?.tenants, []);

newRequest();
let resourceStarted = false;
let listStarted = false;
handle = async operation => {
  if (operation === "session") return session;
  if (operation === "hosting.command") {
    resourceStarted = true;
    await tick();
    throw Error("Resource unavailable");
  }
  if (operation === "listTenants") {
    listStarted = true;
    await tick();
    throw Error("Tenant list unavailable");
  }
  throw Error(operation);
};
await assert.rejects(getConsoleContext("hosting/projects/7/overview"));
await tick();
assert(resourceStarted && listStarted, "parallel failures must both be observed without unhandled rejections");

// Failed reads are never retained beyond the render that encountered the error.
newRequest();
handle = async () => { throw Error("API unavailable"); };
await assert.rejects(registryClient.findByID({ collection: "projects", id: "7" }));
newRequest();
handle = async () => ({ id: "7", name: "Recovered" });
assert.equal((await registryClient.findByID({ collection: "projects", id: "7" })).name, "Recovered");
newRequest();
calls.length = 0;
handle = async (_operation, args) => {
  const [method, options] = args as [string, { page?: number }];
  if (method === "count") return { totalDocs: 2 };
  const page = options.page ?? 1;
  return { docs: [{ id: String(page) }], page, totalPages: 2, hasNextPage: page < 2, nextPage: page < 2 ? 2 : null };
};
const [first, second] = await Promise.all([
  registryClient.find({ collection: "projects", pagination: false }),
  registryClient.find({ collection: "projects", pagination: false }),
]);
assert.deepEqual(first.docs.map(doc => doc.id), ["1", "2"]);
assert.deepEqual(second, first);
assert.equal(calls.length, 2, "unpaginated duplicates share each page while preserving all records");
await Promise.all([
  registryClient.count({ collection: "projects" }),
  registryClient.count({ collection: "projects" }),
]);
assert.equal(calls.length, 3);
internals.A = null;
await registryClient.count({ collection: "projects" });
await registryClient.count({ collection: "projects" });
assert.equal(calls.length, 5, "reads outside a server render remain uncached");

// Render the real async overview component against its one compact API response.
const { createProjectOverviewView } = await import("../../../../modules/hosting/src/ui/project-overview");
const { getProjectOverview } = await import("../../src/lib/project-overview");
const { ProjectOverview } = createProjectOverviewView({ getProjectOverview, applicationState: value => value });
function descendants(node: React.ReactNode): React.ReactElement<Record<string, unknown>>[] {
  if (Array.isArray(node)) return node.flatMap(descendants);
  if (!React.isValidElement<{ children?: React.ReactNode }>(node)) return [];
  return [node as React.ReactElement<Record<string, unknown>>, ...descendants(node.props.children)];
}
function textContent(node: React.ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textContent).join("");
  return React.isValidElement<{ children?: React.ReactNode }>(node) ? textContent(node.props.children) : "";
}
const overview = {
  project: { name: "Overview Project", configured: true, provider: "k3s", canManage: true },
  applications: Array.from({ length: 9 }, (_, i) => ({ id: String(i + 1), name: "App " + (i + 1), lifecycle: "deployed", status: "ready" })),
  databaseCount: 1,
  bindings: [{ id: "21", appID: "1", service: { kind: "postgres", variable: "DATABASE_URL" }, state: "connected" }],
  setups: [{ id: "31", name: "Pending setup", state: "running" }],
  sources: [{ targetID: "1", repositoryName: "owner/repo", branch: "main" }],
};
newRequest();
calls.length = 0;
handle = async (operation, args) => {
  if (operation === "session") return session;
  assert.equal(operation, "hosting.command");
  assert.deepEqual(args, [{ action: "projects.overview", projectID: "7" }]);
  return overview;
};
const rendered = await ProjectOverview({ projectID: "7" });
assert.deepEqual(calls, ["session", "projects.overview"]);
const nodes = descendants(rendered);
const links = nodes.filter(node => node.type === "a").map(node => node.props.href);
for (const href of ["/hosting/projects/7/setup", "/hosting/projects/7/setup/31", "/hosting/projects/7/git?target=1", "/hosting/projects/7/databases", "/hosting/apps/1", "/hosting/apps/8"]) assert(links.includes(href), String(href));
assert(!links.includes("/hosting/apps/9"), "overview retains its eight-application display bound");
for (const text of ["Overview Project", "Pending setup", "In progress", "owner/repo", "main", "DATABASE_URL", "Connected"]) assert(textContent(rendered).includes(text), text);
assert(nodes.some(node => node.props.active === true && node.props.interval === 10000), "running setups retain polling");
await ProjectOverview({ projectID: "7" });
assert.equal(calls.length, 2, "duplicate overview consumers share their read within the render");

for (const pending of [false, true]) {
  newRequest();
  handle = async operation => operation === "session" ? session : ({ ...overview, project: { ...overview.project, canManage: false }, applications: [{ ...overview.applications[0], status: pending ? "pending" : "ready" }], databaseCount: 0, setups: [], bindings: [], sources: [] });
  const result = await ProjectOverview({ projectID: "7" });
  const elements = descendants(result);
  assert(elements.some(node => node.props.active === pending && node.props.interval === 10000));
  const targets = elements.filter(node => node.type === "a").map(node => node.props.href);
  assert(!targets.includes("/hosting/projects/7/setup"), "read-only overview cannot offer application creation");
  assert(!targets.includes("/hosting/projects/7/databases"));
  assert(textContent(result).includes("No services connected to applications yet."));
}
internals.A = null;
