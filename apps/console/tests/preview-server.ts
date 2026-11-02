// Test-only browser fixtures. Production runtime has no fixture or fallback data.
import { createServer } from "vite";
import type { IncomingMessage, ServerResponse } from "node:http";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import { recordSchema } from "../src/api";
import { workbench } from "./workbench-fixtures";
const root = fileURLToPath(new URL("../", import.meta.url));
const records = {
  projects: [
    {
      id: "201",
      name: "Acceptance project",
      customer: "101",
      status: "active",
      url: null,
      repositoryURL: null,
      notes: "",
      createdAt: "2026-10-19T10:00:00Z",
      updatedAt: "2026-10-19T10:00:00Z",
    },
  ],
  customers: [
    {
      id: "101",
      name: "Acceptance customer",
      status: "active",
      customerType: "company",
      contactEmail: "acceptance@example.test",
      notes: "",
      createdAt: "2026-10-19T10:00:00Z",
      updatedAt: "2026-10-19T10:00:00Z",
    },
  ],
  "audit-events": [],
} satisfies Record<string, z.infer<typeof recordSchema>[]>;
// Test-only members per customer.
const members: Record<string, { id: string; role: string; name: string; email: string }[]> = {};
// Test-only tenant invitations per customer.
const tenants: Record<string, { id: string; email: string; role: string; status: string; createdAt: string; progress: { emailDelivered: string; accountReady: boolean; joinedAt: null } }[]> = {};
const activity: Record<string, { id: string; action: string; outcome: string; createdAt: string; actorName: string }[]> = {};
const bodySchema = recordSchema
  .omit({ id: true })
  .extend({ archived: z.boolean().optional() });
async function registry(req: IncomingMessage, url: URL) {
  const [, , , , collection, id] = url.pathname.split("/");
  if (!collection || !Object.hasOwn(records, collection)) return undefined;
  const list: z.infer<typeof recordSchema>[] =
    records[collection as keyof typeof records];
  if (req.method === "GET")
    return id
      ? list.find((row) => row.id === id)
      : {
          docs: list,
          page: 1,
          totalPages: 1,
          totalDocs: list.length,
          hasNextPage: false,
          hasPrevPage: false,
        };
  if (req.method === "DELETE") {
    // Like Core: only archived customers can be deleted.
    const index = list.findIndex((row) => row.id === id && row.status === "archived");
    if (collection !== "customers" || index < 0) return undefined;
    list.splice(index, 1);
    return { id, deleted: true };
  }
  const chunks: Buffer[] = [];
  for await (const chunk of req) {
    const value: unknown = chunk;
    if (!Buffer.isBuffer(value))
      throw new Error("Expected a request body buffer");
    chunks.push(value);
  }
  const body = bodySchema.parse(JSON.parse(Buffer.concat(chunks).toString()));
  const previous = id ? list.find((row) => row.id === id) : undefined;
  if (req.method === "PATCH" && previous) {
    previous.status = body.archived ? "archived" : "active";
    return previous;
  }
  if (previous) {
    Object.assign(previous, body);
    return previous;
  }
  const created = recordSchema.parse({
    ...body,
    id: String(1000 + list.length),
    status: "active",
  });
  list.push(created);
  return created;
}
// The seeded customer already has its owner and one administrator.
function peopleOf(customer: string) {
  return (members[customer] ??= customer === "101" ? [
    { id: "6001", role: "owner", name: "Ada Owner", email: "ada@example.test" },
    { id: "6002", role: "admin", name: "Grace Admin", email: "grace@example.test" },
  ] : []);
}
function memberChange(people: (typeof members)[string], command: { action: string; member?: string | undefined; role?: string | undefined }) {
  const person = people.find((row) => row.id === command.member);
  if (person && command.action === "role") person.role = command.role ?? person.role;
  if (person && command.action === "remove") people.splice(people.indexOf(person), 1);
  return { id: command.member, message: "Change saved." };
}
async function tenant(req: IncomingMessage, url: URL) {
  const match = /^\/api\/console\/tenant\/([0-9]+)$/.exec(url.pathname);
  if (!match) return undefined;
  const invitations = (tenants[match[1] ?? ""] ??= []);
  const events = (activity[match[1] ?? ""] ??= []);
  const people = peopleOf(match[1] ?? "");
  if (req.method !== "POST") return { canManage: true, operator: true, tenant: { member_role: null }, members: people, invitations, activity: events };
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(Buffer.from(chunk as Buffer));
  const command = z.object({ action: z.string(), email: z.string().optional(), role: z.string().optional(), invitation: z.string().optional(), member: z.string().optional() }).parse(JSON.parse(Buffer.concat(chunks).toString()));
  if (command.action === "role" || command.action === "remove") return memberChange(people, command);
  if (command.action === "invite" && command.email) invitations.push({ id: String(5000 + invitations.length), email: command.email, role: command.role ?? "admin", status: "pending", createdAt: new Date().toISOString(), progress: { emailDelivered: "queued", accountReady: false, joinedAt: null } });
  events.unshift({ id: String(7000 + events.length), action: `tenant-${command.action}`, outcome: "succeeded", createdAt: new Date().toISOString(), actorName: "Acceptance operator" });
  const target = command.invitation ?? invitations.at(-1)?.id ?? "0";
  if (command.action === "cancel") invitations.forEach((row) => { if (row.id === target) row.status = "canceled"; });
  return { id: target, message: "Invitation saved and queued. Access starts after acceptance.", delivery: "queued", ...(command.action === "cancel" ? {} : { link: `${url.origin}/reset-password?token=${"a".repeat(64)}&invitation=${target}` }) };
}
async function serve(
  req: IncomingMessage,
  res: ServerResponse,
  next: () => void,
) {
  const url = new URL(req.url ?? "/", `http://${req.headers.host ?? "127.0.0.1"}`);
  const send = (data: unknown) => {
    res.setHeader("Content-Type", "application/json");
    res.setHeader("Cache-Control", "no-store");
    res.end(JSON.stringify(data));
  };
  if (url.pathname === "/console-config.json") {
    send({
      apiOrigin: url.origin,
      authOrigin: url.origin,
      enabledFeatures: ["hosting"],
    });
    return;
  }
  if (url.pathname === "/api/console/plugins") {
    send([]);
    return;
  }
  if (url.pathname === "/api/console/plugins/catalog") { send([]); return; }
  if (url.pathname === "/v1/auth/session") {
    send({
      data: {
        user: {
          id: "1",
          name: "Acceptance operator",
          email: "operator@example.test",
          role: "operator",
          emailVerified: true,
        },
        session: {
          id: "test-session",
          userId: "1",
          expiresAt: "2026-10-20T10:00:00Z",
        },
      },
    });
    return;
  }
  const tenantResult = await tenant(req, url);
  if (tenantResult) {
    send(tenantResult);
    return;
  }
  const result = await workbench(req, url);
  if (result) {
    res.statusCode = result.status;
    res.setHeader("Content-Type", "application/json");
    res.setHeader("Cache-Control", "no-store");
    res.end(await result.text());
    return;
  }
  if (!url.pathname.startsWith("/v1/operator/registry/")) {
    next();
    return;
  }
  const data = await registry(req, url);
  if (!data) {
    res.statusCode = 404;
    res.end();
    return;
  }
  send(data);
}
export const server = await createServer({
  root,
  cacheDir: fileURLToPath(new URL('../../../.runtime-test/vite-cache/preview', import.meta.url)),
  // 3121 by default; another free port when a local Tilt Console already uses it.
  server: { host: "127.0.0.1", port: Number(process.env.WEBDOCK_PREVIEW_PORT ?? 3121), strictPort: false },
  plugins: [
    {
      name: "test-only-api",
      configureServer(server) {
        server.middlewares.use((req, res, next) => {
          void serve(req, res, next).catch(next);
        });
      },
    },
  ],
});
await server.listen();
export const previewOrigin = (server.resolvedUrls?.local[0] ?? "http://127.0.0.1:3121/").replace(/\/$/, "");
console.log(`Test-only preview listening on ${previewOrigin}`);
