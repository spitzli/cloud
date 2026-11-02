// HTTP fixtures only for the isolated browser acceptance server.
import type { IncomingMessage } from "node:http";
import { z } from "zod";
import { commandSchema } from "@webdock/hosting-contracts";
import { appSchema } from "../src/hosting-contracts";
import {
  installationSchema,
  stateRequest,
} from "../src/plugins/lifecycle-contracts";
type Mode =
  "normal" | "conflict" | "uncertain" | "denied" | "failed" | "blocked";
let mode: Mode = "normal";
export function setWorkbenchMode(value: Mode) {
  mode = value;
}
export const workbenchRequests: { path: string; body: unknown }[] = [];
let application = appSchema.parse({
  id: "301",
  name: "Acceptance application",
  projectID: "201",
  lifecycle: "deployed",
  revision: 1,
  observedRevision: 1,
  status: "ready",
  operationID: null,
  lastError: null,
  logs: "Previous recorded log output",
  environmentNames: ["EXAMPLE_TOKEN"],
  subscriptionRevision: 3,
  operator: true,
  canManage: true,
  canRequestLogs: true,
  spec: {
    template: "custom",
    image: `registry.example.test/app@sha256:${"a".repeat(64)}`,
    args: [],
    port: 8080,
    healthPath: "/",
    cpuMillicores: 100,
    memoryBytes: 67108864,
    ephemeralBytes: 67108864,
    replicas: 1,
    volumeBytes: 0,
  },
});
let extension = installationSchema.parse({
  id: "acceptance-extension",
  kind: "external",
  revision: "1",
  desiredState: "enabled",
  state: "enabled",
  version: "1.0.0",
  imageDigest: null,
  runtime: "ready",
  blockers: [],
  dependencies: [],
});
interface Operation {
  id: string;
  action: string;
  status: string;
  reads: number;
  terminal: "succeeded" | "failed" | "blocked";
}
const operations = new Map<string, Operation>();
const receipts = new Map<string, { body: string; receipt: object }>();
const json = (value: unknown, status = 200) => Response.json(value, { status });
async function input(req: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) {
    const value: unknown = chunk;
    if (!Buffer.isBuffer(value)) throw new Error("Invalid fixture input");
    chunks.push(value);
  }
  const value: unknown = JSON.parse(Buffer.concat(chunks).toString());
  return value;
}
function rejectNext() {
  if (mode === "denied") {
    mode = "normal";
    return json({ error: "Fixture access denied" }, 403);
  }
  if (mode === "conflict") {
    mode = "normal";
    return json({ error: "Fixture revision conflict" }, 409);
  }
  return undefined;
}
function operation(id: string) {
  const value = operations.get(id);
  if (!value) throw new Error("Unknown fixture operation");
  value.reads++;
  value.status = value.reads >= 2 ? value.terminal : "running";
  if (value.status === "succeeded" && value.action.startsWith("apps.")) {
    application.observedRevision = application.revision;
    application.status = application.spec.replicas ? "ready" : "stopped";
    if (value.action === "apps.logs")
      application.logs = "Fresh sanitized fixture log output";
  }
  if (value.status === "failed") {
    application.status = "failed";
    application.lastError = "The fixture runtime rejected the request.";
  }
  if (value.status === "succeeded" && value.action === "state")
    extension.state = extension.desiredState;
  return value;
}
function newOperation(action: string): Operation {
  const value: Operation = {
    id: String(800 + operations.size),
    action,
    status: "queued",
    reads: 0,
    terminal:
      mode === "failed"
        ? "failed"
        : mode === "blocked"
          ? "blocked"
          : "succeeded",
  };
  operations.set(value.id, value);
  return value;
}
function hosting(body: unknown): Response {
  const command = commandSchema.parse(body);
  if (command.action === "projects.list")
    return json({
      docs: [
        {
          projectID: "201",
          name: "Acceptance hosting project",
          provider: "docker",
          mode: "selfservice",
          ownImages: true,
          revision: 1,
          clusterID: "401",
          clusterName: "Acceptance cluster",
          country: null,
          region: null,
        },
      ],
      page: 1,
      totalPages: 1,
      totalDocs: 1,
      limit: 20,
      operator: true,
    });
  if (command.action === "projects.get")
    return json({
      projectID: "201",
      name: "Acceptance hosting project",
      customerID: "101",
      customerName: "Acceptance customer",
      operator: true,
      configured: true,
      provider: "docker",
    });
  if (command.action === "apps.list")
    return json({
      docs: [application],
      page: 1,
      totalPages: 1,
      totalDocs: 1,
      limit: 20,
      operator: true,
      ownImages: true,
      canManage: true,
      subscriptionRevision: 3,
    });
  if (command.action === "apps.get") return json(application);
  if (command.action === "operations.get") {
    const value = operation(command.operationID);
    return json({
      id: value.id,
      action: value.action,
      status: value.status,
      customerID: "101",
      projectID: "201",
      createdAt: "2026-10-20T10:00:00Z",
    });
  }
  if (!("idempotencyKey" in command))
    return json({ error: "Unsupported fixture action" }, 400);
  const replay = receipts.get(command.idempotencyKey);
  if (replay)
    return replay.body === JSON.stringify(command)
      ? json(replay.receipt)
      : json({ error: "Changed fixture payload" }, 409);
  const failure = rejectNext();
  if (failure) return failure;
  if ("revision" in command && command.revision !== application.revision)
    return json({ error: "Fixture revision changed" }, 409);
  if (command.action === "apps.update")
    application = { ...application, spec: command.spec };
  if (command.action === "apps.scale")
    application.spec.replicas = command.replicas;
  if (command.action === "apps.stop") application.spec.replicas = 0;
  if (command.action === "apps.start") application.spec.replicas = 1;
  const value = newOperation(command.action);
  if (command.action !== "apps.logs") {
    application.revision++;
    application.status = "pending";
    application.operationID = value.id;
  }
  const receipt = {
    id: value.id,
    appID: application.id,
    status: "queued",
    revision: application.revision,
  };
  receipts.set(command.idempotencyKey, {
    body: JSON.stringify(command),
    receipt,
  });
  if (mode === "uncertain") {
    mode = "normal";
    return json({ error: "Fixture response lost after acceptance" }, 503);
  }
  mode = "normal";
  return json(receipt);
}
const stateInput = z
  .object({
    action: z.literal("state"),
    pluginID: z.string(),
    expectedRevision: z.string(),
    desired: z.enum(["enabled", "draining", "disabled"]),
    idempotencyKey: z.string(),
  })
  .strict();
function changeExtension(body: unknown): Response {
  const input = stateInput.parse(body);
  const command = stateRequest(
    input.pluginID,
    input.expectedRevision,
    input.desired,
    input.idempotencyKey,
  );
  const replay = receipts.get(command.idempotencyKey);
  if (replay)
    return replay.body === JSON.stringify(command)
      ? json(replay.receipt)
      : json({}, 409);
  const failure = rejectNext();
  if (failure) return failure;
  if (command.expectedRevision !== extension.revision) return json({}, 409);
  const value = newOperation("state");
  extension = {
    ...extension,
    desiredState: command.desired,
    revision: String(Number(extension.revision) + 1),
  };
  const receipt = { id: value.id, status: "pending" };
  receipts.set(command.idempotencyKey, {
    body: JSON.stringify(command),
    receipt,
  });
  if (mode === "uncertain") {
    mode = "normal";
    return json(
      { error: "Fixture availability response lost after acceptance" },
      503,
    );
  }
  mode = "normal";
  return json(receipt);
}
export async function workbench(
  req: IncomingMessage,
  url: URL,
): Promise<Response | undefined> {
  if (url.pathname === "/api/console/tenants")
    return json([
      {
        id: "101",
        name: "Acceptance customer",
        status: "active",
        role: "admin",
      },
    ]);
  if (url.pathname === "/api/console/sites") return json([]);
  if (url.pathname === "/api/console/hosting") {
    const body = await input(req);
    workbenchRequests.push({ path: url.pathname, body });
    return hosting(body);
  }
  if (url.pathname.startsWith("/api/console/hosting/reads/")) {
    const value = operation(url.pathname.split("/").at(-1) ?? "");
    return json({
      id: value.id,
      status: value.status,
      result:
        value.status === "succeeded"
          ? { logs: "Fresh sanitized fixture log output", truncated: true }
          : null,
      error: value.status === "failed" ? "Log read failed" : null,
    });
  }
  if (url.pathname === "/api/console/plugins/installed") {
    if (req.method === "GET") return json([extension]);
    const body = await input(req);
    workbenchRequests.push({ path: url.pathname, body });
    return changeExtension(body);
  }
  if (url.pathname.startsWith("/api/console/plugins/operations/")) {
    const value = operation(url.pathname.split("/").at(-1) ?? "");
    return json({
      id: value.id,
      pluginID: extension.id,
      status: value.status,
      phase:
        value.status === "blocked"
          ? "draining"
          : value.status === "succeeded"
            ? "complete"
            : "checking",
      generation: "2",
      cursor: String(value.reads),
      condition: value.status === "blocked" ? "active_continuations" : null,
    });
  }
  return undefined;
}
