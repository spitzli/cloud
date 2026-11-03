// Synthetic transport is confined to this explicit test entry. No Neon account is contacted.
import { useState } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@webdock/i18n/react";
import Neon from "../../../plugins/neon/ui/index";
import { ApiClient, type Session } from "../src/api";
import { ApiContext, SessionContext } from "../src/context";
import { Brand } from "../src/AuthFrame";
import type { PluginResource } from "../src/plugins/sdk";
import * as ui from "../src/ui";
import "../src/styles.css";

const parameters = new URLSearchParams(location.search);
const initialView = parameters.get("view") ?? "create";
const locale = parameters.get("locale") === "de" ? "de" : "en";
const writable = initialView !== "read-only";
const resource: PluginResource = { id: "bound-synthetic-1", offering: "database", name: "Store database", createdVia: "self-service" };
const session: Session = {
  user: { id: "42", name: "Demo customer", email: "demo@example.test", role: "user", emailVerified: true, customerScope: "101" },
  session: { id: "synthetic-customer-session", userId: "42", expiresAt: "2099-01-01T00:00:00Z" },
};
const projects = [
  { id: "neon-store", name: "Demo store", region_id: "aws-eu-central-1" },
  { id: "neon-us", name: "US sample (unavailable)", region_id: "aws-us-east-1" },
  { id: "neon-london", name: "London sample (unavailable)", region_id: "aws-eu-west-2" },
];
const branches = [{ id: "br-main", name: "production" }, { id: "br-preview", name: "preview" }];
const syntheticURI = "postgresql://demo_owner:synthetic-only-password@ep-demo-pooler.eu-central-1.aws.neon.tech/store?sslmode=require";
const panel = {
  version: 2,
  title: "Store database",
  blocks: [
    { kind: "status", label: "Compute", state: "healthy", text: "Neon compute is active" },
    { kind: "table", title: "Connection settings", columns: [{ key: "field", label: "Setting" }, { key: "value", label: "Value" }], rows: [
      { field: "Neon project", value: "Demo store" }, { field: "Branch", value: "production" },
      { field: "Database", value: "store" }, { field: "Role", value: "demo_owner" },
      { field: "Region", value: "Frankfurt, Germany · aws-eu-central-1" },
      { field: "Endpoint", value: "ep-demo · read_write · active" },
    ] },
  ],
};
const branchPanel = {
  version: 2, title: "Branches", blocks: [{ kind: "table", title: "Neon branches", columns: [{ key: "name", label: "Branch" }, { key: "state", label: "Compute" }], rows: [{ name: "production", state: "active" }, { name: "preview", state: "idle" }] }],
};
interface Invocation { pluginID: string; projectID: string; action: string; input: Record<string, unknown>; idempotencyKey: string }
const requests: Invocation[] = [];
const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false }, mutations: { retry: false } } });
let providerUnavailable = initialView === "failed";
let holdCredentials = initialView === "pending";
let credentialAborts = 0;
let pendingCredentials: (() => void)[] = [];
let attachAttempts = 0;
let detached = false;
let attachedRequest: Invocation | undefined;
const json = (value: unknown, status = 200) => Response.json(value, { status, headers: { "Cache-Control": "no-store" } });

const fakeFetcher: typeof fetch = async (input, init) => {
  const url = new URL(input instanceof Request ? input.url : String(input), location.origin);
  if (url.origin !== location.origin || url.pathname !== "/api/console/plugins/invoke" || init?.method !== "POST" || typeof init.body !== "string") throw new Error("Fixture refuses non-plugin requests");
  const command = JSON.parse(init.body) as Invocation;
  if (command.pluginID !== "neon" || command.projectID !== "201" || !command.idempotencyKey) throw new Error("Unexpected plugin scope");
  requests.push(command);
  if (!writable && command.action !== "databases.view") return json({ error: "Read-only customer" }, 403);
  switch (command.action) {
    case "databases.discover": {
      const selectedProject = command.input.neon_project_id;
      const selectedBranch = command.input.branch_id;
      if (selectedProject && selectedProject !== "neon-store") return json({ error: "Region unavailable" }, 400);
      const validBranch = branches.some(branch => branch.id === selectedBranch);
      return json({ projects, branches: selectedProject ? branches : [], databases: validBranch ? [{ name: "store" }, { name: "analytics" }] : [], roles: validBranch ? [{ name: "demo_owner" }, { name: "app_user" }] : [], endpoints: validBranch ? [{ id: selectedBranch === "br-main" ? "ep-demo" : "ep-preview", branch_id: selectedBranch, type: "read_write", current_state: "active" }] : [], cursor: null });
    }
    case "databases.attach": {
      const tuple = command.input;
      if (tuple.neon_project_id !== "neon-store" || tuple.branch_id !== "br-main" || tuple.database_name !== "store" || tuple.role_name !== "demo_owner" || tuple.endpoint_id !== "ep-demo" || typeof tuple.name !== "string" || !tuple.name.trim()) return json({ error: "Invalid attachment tuple" }, 400);
      attachAttempts++;
      // Simulate a committed attachment whose first response was lost. Retry must keep its key.
      if (attachedRequest && (attachedRequest.idempotencyKey !== command.idempotencyKey || JSON.stringify(attachedRequest.input) !== JSON.stringify(command.input))) return json({ error: "Idempotency conflict" }, 409);
      attachedRequest = command;
      if (parameters.get("retry") === "1" && attachAttempts === 1) return json({ error: "Synthetic response lost" }, 503);
      return json({ resource: { id: resource.id, name: tuple.name } });
    }
    case "databases.view": return providerUnavailable ? json({ error: "Synthetic provider outage" }, 503) : json(panel);
    case "databases.branches": return providerUnavailable ? json({ error: "Synthetic provider outage" }, 503) : json(branchPanel);
    case "databases.credentials": {
      if (providerUnavailable) return json({ error: "Synthetic provider outage" }, 503);
      const value = command.input.pooled === false ? syntheticURI.replace("ep-demo-pooler", "ep-demo") : syntheticURI;
      if (!holdCredentials) return json({ value });
      // Deliberately allow a late transport response after abort to test the real SDK's discard guard.
      init.signal?.addEventListener("abort", () => { credentialAborts++; }, { once: true });
      return new Promise<Response>(resolve => { pendingCredentials.push(() => { resolve(json({ value })); }); });
    }
    case "databases.detach": {
      if (command.input.resource_id !== resource.id) return json({ error: "Unknown attachment" }, 404);
      detached = true;
      return json({ detached: true, resource_id: resource.id });
    }
    default: throw new Error("Fixture refuses an undeclared plugin action");
  }
};
const api = new ApiClient({ apiOrigin: location.origin, authOrigin: location.origin, enabledFeatures: [] }, fakeFetcher);

// Test-only controls and read-only diagnostics. Reveal results never enter this shared object.
const fixture = {
  diagnostics: () => ({ requests: structuredClone(requests), sharedCache: JSON.stringify([client.getQueryCache().getAll().map(query => query.state), client.getMutationCache().getAll().map(mutation => mutation.state)]), credentialAborts, pendingCredentials: pendingCredentials.length, detached, sessionRole: session.user.role, writable }),
  setProviderUnavailable: async () => { providerUnavailable = true; await client.invalidateQueries({ queryKey: ["plugin-ui", "neon", "201"] }); },
  holdCredentials: () => { holdCredentials = true; },
  completeCredentials: async () => { const pending = pendingCredentials; pendingCredentials = []; holdCredentials = false; pending.forEach(resolve => { resolve(); }); await new Promise(resolve => { setTimeout(resolve, 0); }); },
};
declare global { interface Window { __neonPluginFixture: typeof fixture } }
window.__neonPluginFixture = fixture;

function Fixture() {
  const [view, setView] = useState(initialView === "create" ? "create" : "resource");
  const [name, setName] = useState(resource.name);
  const Create = Neon.Create["databases.attach"];
  const Resource = Neon.Resource;
  return <div className="min-h-dvh">
    <header className="flex flex-wrap items-center justify-between gap-3 border-b border-outline bg-surface px-5 py-4 max-sm:px-4">
      <Brand /><span className="text-sm text-muted">Demo workspace · Customer</span>
    </header>
    <div className="border-b border-outline bg-subtle px-5 py-3 text-center text-xs text-muted" role="note">Synthetic acceptance fixture · Demo data only · No Neon account is contacted</div>
    <main className="mx-auto grid w-full max-w-3xl min-w-0 gap-6 px-6 py-8 max-sm:px-4">
      <nav aria-label="Breadcrumb" className="flex flex-wrap gap-2 text-sm text-muted"><span>Demo workspace</span><span aria-hidden> / </span><span>Store project</span><span aria-hidden> / </span><span>Databases</span></nav>
      <div><h1 className={ui.pageTitle}>{view === "create" ? "Connect Neon database" : view === "detached" ? "Database detached" : view === "cancelled" ? "Connection cancelled" : name}</h1><p className={ui.pageLead}>{view === "create" ? "Use your existing Neon database in this project." : "Your Neon database, connected to Spitzli Cloud."}</p></div>
      {!writable ? <p className={ui.notice}>Read-only view · Your workspace role can inspect connection settings.</p> : null}
      {view === "create" ? <section className={ui.card}><div className={ui.cardBody}><Create plugin="neon" projectID="201" customer="101" action="databases.attach" title="Connect Neon database" resources={[]} onCreated={() => { const attachedName = attachedRequest?.input.name; setName(typeof attachedName === "string" ? attachedName : resource.name); setView("resource"); }} onCancel={() => { setView("cancelled"); }} /></div></section> : null}
      {view === "resource" ? <Resource plugin="neon" projectID="201" customer="101" resource={{ ...resource, name }} features={[]} writable={writable} actions={["databases.view", "databases.branches", "databases.credentials", "databases.detach"]} onDeleted={() => { setView("detached"); }} /> : null}
      {view === "detached" ? <p className={ui.notice} role="status">The local attachment was removed. Your database, branches and data remain in Neon.</p> : null}
      {view === "cancelled" ? <p className={ui.notice} role="status">No database was attached.</p> : null}
    </main>
  </div>;
}
const root = document.getElementById("root");
if (!root) throw new Error("Fixture root is missing");
createRoot(root).render(<QueryClientProvider client={client}><I18nProvider locale={locale} preference={locale}><ApiContext.Provider value={api}><SessionContext.Provider value={session}><Fixture /></SessionContext.Provider></ApiContext.Provider></I18nProvider></QueryClientProvider>);
