// Local HTTP fixture only. Never imported by the Console application or Core.
import type { IncomingMessage } from "node:http";
import { z } from "zod";
import postgres from "@plugins/postgres/manifest.json";
import mail from "@plugins/mail/manifest.json";

export const customerTenants = [
  { id: "101", name: "Nordlicht GmbH", status: "active", role: "owner" },
  { id: "102", name: "Studio Linden", status: "active", role: "member" },
];
export const customerProjects = [
  { id: "201", customer: "101", name: "Onlineshop", status: "active" },
  { id: "202", customer: "101", name: "Kundenportal", status: "active" },
  { id: "203", customer: "101", name: "Website und Veranstaltungen für Geschäftskunden", status: "active" },
  { id: "301", customer: "102", name: "Kampagne", status: "active" },
];
const resource = (id: string, name: string, offering = "shared") => ({ id, name, offering, createdVia: "self-service" as const });
const databases: Record<string, ReturnType<typeof resource>[]> = {
  "201": [resource("00000000-0000-4000-8000-000000000001", "production")],
  "202": [resource("00000000-0000-4000-8000-000000000002", "production", "dedicated")],
  "301": [resource("00000000-0000-4000-8000-000000000003", "studio-only", "dedicated")],
};
const meters = [{ id: "projects", label: "Projects", unit: "count" }, { id: "memory", label: "Memory", unit: "MiB" }, { id: "cpu", label: "CPU", unit: "millicores" }, { id: "storage", label: "Storage", unit: "MiB" }];
const limits = { projects: 10, memory: 4096, cpu: 2000, storage: 20480 };
const plan = { id: "401", name: "Business", description: "Local synthetic customer plan", assigned: true };
const user = { id: "2", name: "Ada Beispiel", email: "ada@example.test", role: "user", emailVerified: true, twoFactorEnabled: true };
const session = { user, session: { id: "customer-fixture", userId: user.id, expiresAt: "2099-12-01T00:00:00Z" } };
const state = { failCreate: false, failedProjects: [] as string[], calls: [] as { path: string; method: string; project?: string }[] };
const json = (data: unknown, status = 200) => Response.json(data, { status });
const commandSchema = z.object({ action: z.string().optional(), customerID: z.string().optional(), projectID: z.string().optional(), input: z.record(z.string(), z.unknown()).optional() });
async function body(req: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(Buffer.from(chunk as Buffer));
  return JSON.parse(Buffer.concat(chunks).toString());
}
export async function customerFixture(req: IncomingMessage, url: URL): Promise<Response | undefined> {
  const path = url.pathname;
  if (path === "/__customer-fixture") {
    if (req.method === "POST") {
      const change = z.object({ failedProjects: z.array(z.string()).optional(), failCreate: z.boolean().optional() }).strict().parse(await body(req));
      Object.assign(state, change);
    }
    return json(state);
  }
  if (path === "/console-config.json") return json({ apiOrigin: url.origin, authOrigin: url.origin, enabledFeatures: [], localePreference: String(req.headers.cookie).includes("fixture-language=de") ? "de" : "en" });
  if (path === "/api/locale") {
    const preference = z.object({ preference: z.enum(["en", "de", "system"]) }).parse(await body(req));
    return new Response("{}", { headers: { "Content-Type": "application/json", "Set-Cookie": `fixture-language=${preference.preference}; Path=/; SameSite=Lax` } });
  }
  if (!/^\/(api|v1)\//.test(path)) return undefined;
  state.calls.push({ path, method: req.method ?? "GET", ...(url.searchParams.has("projectID") ? { project: url.searchParams.get("projectID") ?? "" } : {}) });
  if (path.startsWith("/v1/operator/")) return json({ error: { message: "Operator only." } }, 403);
  const identity = identityFixture(path);
  if (identity) return identity;
  const projects = /^\/api\/console\/tenant\/(\d+)\/projects$/.exec(path);
  if (projects) {
    if (req.method === "POST") return createProject(req, projects[1] ?? "");
    return json(customerProjects.filter(p => p.customer === projects[1]));
  }
  const project = /^\/api\/console\/project\/(\d+)$/.exec(path);
  if (project) {
    const item = customerProjects.find(p => p.id === project[1]);
    if (!item) return json({ error: { message: "Project not found." } }, 404);
    const tenant = customerTenants.find(t => t.id === item.customer);
    return json({ ...item, customer: tenant, role: tenant?.role });
  }
  const plugin = await pluginFixture(req, url);
  if (plugin) return plugin;
  const workspace = await workspaceFixture(req, path);
  if (workspace) return workspace;
  return json({ error: { message: `Unimplemented customer fixture: ${path}` } }, 404);
}

function identityFixture(path: string): Response | undefined {
  if (path === "/v1/auth/session") return json({ data: session });
  if (path === "/api/auth/get-session") return json(session);
  if (path === "/api/auth/list-sessions") return json([{ ...session.session, current: true, createdAt: "2026-11-01T10:00:00Z" }]);
  if (path === "/api/auth/passkey/list-user-passkeys" || path === "/api/invitations") return json([]);
  if (["/api/console/tenants", "/api/tenants"].includes(path)) return json(customerTenants);
  return undefined;
}

async function pluginFixture(req: IncomingMessage, url: URL): Promise<Response | undefined> {
  const path = url.pathname;
  if (path === "/api/console/plugins") {
    const id = url.searchParams.get("projectID") ?? "";
    if (state.failedProjects.includes(id)) return json({ error: { message: "Fixture resource inventory temporarily unavailable." } }, 503);
    const owner = customerProjects.find(p => p.id === id)?.customer === "101";
    return json([
      { state: "enabled", runtime: "ready", manifest: postgres, features: owner ? ["create-shared", "create-dedicated"] : [], resources: databases[id] ?? [] },
      { state: "enabled", runtime: "ready", manifest: mail, features: owner ? ["add-domains"] : [], resources: id === "201" ? [resource("00000000-0000-4000-8000-0000000000d1", "shop.example.test", "domain")] : [] },
    ]);
  }
  if (path === "/api/console/plugins/invoke") {
    const command = commandSchema.parse(await body(req));
    const item = databases[command.projectID ?? ""]?.find(r => r.id === command.input?.resource_id);
    if (command.action === "databases.view" && item) return json({ ...item, state: "ready", host: "db.example.test", port: 5432, database: "app", user: "fixture_user", version: "18", server: null, error: null, createdAt: "2026-11-01T10:00:00Z", sslMode: "require" });
    if (command.action === "mail.history") return json({ items: [] });
    if (command.action === "domains.view") return json({ domain: "shop.example.test", verified: false, verificationSkipped: false, transport: "platform", records: [{ type: "TXT", name: "_webdock.shop.example.test", value: "fixture-verification", purpose: "Fixture DNS proof" }], mailboxes: [], recent: [], incoming: { state: "disabled", desiredEnabled: false } });
    return json({ error: { message: "This fixture does not perform provider operations." } }, 400);
  }
  return undefined;
}

async function workspaceFixture(req: IncomingMessage, path: string): Promise<Response | undefined> {
  if (path.endsWith("/placement")) return json({ cluster: null, options: [], movable: true });
  if (path.endsWith("/usage")) {
    const customer = path.split("/").at(-2);
    return json({ plan, limits, overrides: {}, allocated: { projects: customer === "101" ? 3 : 1, memory: 768, cpu: 300, storage: 2048 }, meters, units: [], history: [] });
  }
  if (path.endsWith("/logs")) return json([]);
  if (path.endsWith("/notifications")) return json([]);
  if (path.endsWith("/self-service") || path === "/api/console/plugins/installed") return json([]);
  if (path === "/api/console/byok/neon") {
    const command = commandSchema.parse(await body(req));
    return json({ data: { canManage: command.customerID === "101", connection: null } });
  }
  const tenant = /^\/api\/console\/tenant\/(\d+)$/.exec(path);
  if (tenant) return json({ canManage: tenant[1] === "101", operator: false, tenant: { member_role: tenant[1] === "101" ? "owner" : "member" }, members: [{ ...user, role: "owner" }], invitations: [], activity: [] });
  if (path.endsWith("/domains")) return json([]);
  if (path.endsWith("/api-tokens")) return json([]);
  if (path === "/api/console/events") return new Response("", { headers: { "Content-Type": "text/event-stream" } });
  return undefined;
}

async function createProject(req: IncomingMessage, customer: string): Promise<Response> {
  if (customerTenants.find(tenant => tenant.id === customer)?.role !== "owner") return json({ error: { message: "Workspace administrator access required." } }, 403);
  if (state.failCreate) return json({ error: { message: "Fixture project quota reached. No project was created." } }, 409);
  const input = z.object({ name: z.string().trim().min(1).max(120) }).strict().parse(await body(req));
  const project = { id: String(900 + customerProjects.length), customer, name: input.name, status: "active" };
  customerProjects.push(project);
  return json({ id: project.id }, 201);
}
