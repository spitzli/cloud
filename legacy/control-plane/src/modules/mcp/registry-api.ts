import { projectHostingStatus } from "./product-contributions";
import { RegistryError as APIError } from "../registry/service";
import { z } from "zod";
import { authenticateMCP, authorizeRegistry, mcpResource } from "./mcp-auth";
import {
  collectionName,
  customerInput,
  projectInput,
  listInput,
  recordID,
  listRecords,
  getRecord,
  writeCustomer,
  writeProject,
  setArchived,
} from "../registry/compat";

export async function readJSON(request: Request) {
  if (
    request.headers.get("content-type")?.split(";")[0].trim() !==
    "application/json"
  )
    throw new APIError("Use application/json.", 415);
  const reader = request.body?.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  if (reader)
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        size += value.byteLength;
        if (size > 65536) {
          await reader.cancel();
          throw new APIError("Request is too large.", 413);
        }
        chunks.push(value);
      }
    } finally {
      reader.releaseLock();
    }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8")) as unknown;
  } catch {
    throw new APIError("Invalid JSON.", 400);
  }
}
const json = (data: unknown, status = 200) =>
  Response.json(data, { status, headers: { "Cache-Control": "no-store" } });
const methodNotAllowed = (allow: string) =>
  new Response(null, {
    status: 405,
    headers: { Allow: allow, "Cache-Control": "no-store" },
  });

export async function handleRegistryRequest(
  request: Request,
  path: string[],
  dependencies: {
    authenticate?: typeof authenticateMCP;
  },
) {
  try {
    const access = await authorizeRegistry(
      request,
      request.method !== "GET",
      dependencies,
    );
    if (access instanceof Response) return access;
    const { actor } = access;
    if (!path.length)
      return request.method === "GET"
        ? json({
            resource: mcpResource(),
            scopes: ["webdock:read", "webdock:write"],
            collections: collectionName.options,
            operations: {
              ...(projectHostingStatus ? { hosting: "GET /api/registry/projects/{id}/hosting" } : {}),
              GET: "/api/registry/{collection}[/{id}]",
              POST: "/api/registry/{collection}",
              PUT: "/api/registry/{collection}/{id}",
              PATCH: "customers or projects /{id}: { archived: boolean }",
            },
            schemas: {
              customer: z.toJSONSchema(customerInput),
              project: z.toJSONSchema(projectInput),
              filters: z.toJSONSchema(listInput),
            },
            notes:
              "Bearer authorization only. Uses the same resource and scopes as MCP. Creates are not idempotent; do not retry ambiguous writes automatically.",
          })
        : methodNotAllowed("GET");
    if (
      projectHostingStatus &&
      path.length === 3 &&
      path[0] === "projects" &&
      path[2] === "hosting" &&
      request.method === "GET"
    )
      return json(await projectHostingStatus!(actor, recordID.parse(path[1])));
    if (path.length > 2) return json({ error: "Not found" }, 404);
    const collection = collectionName.parse(path[0]);
    const id = path[1] === undefined ? null : recordID.parse(path[1]);
    if (request.method === "GET") {
      if (id) return json(await getRecord(actor, collection, id));
      const params = new URL(request.url).searchParams;
      const input: Record<string, unknown> = {};
      for (const [key, value] of params) {
        if (params.getAll(key).length !== 1 || key === "collection")
          throw new APIError("Invalid query parameters.", 400);
        input[key] = ["page", "limit"].includes(key)
          ? /^[0-9]+$/.test(value)
            ? Number(value)
            : NaN
          : value;
      }
      return json(
        await listRecords(actor, listInput.parse({ ...input, collection })),
      );
    }
    if (collection === "audit-events") return methodNotAllowed("GET");
    if (
      request.method === "PATCH" &&
      id &&
      (collection === "customers" || collection === "projects")
    ) {
      const { archived } = z
        .object({ archived: z.boolean() })
        .strict()
        .parse(await readJSON(request));
      return json(await setArchived(actor, collection, id, archived));
    }
    if (!(
      (request.method === "POST" && !id) ||
      (request.method === "PUT" && id)
    ))
      return methodNotAllowed(id ? "GET, PUT, PATCH" : "GET, POST");
    const body = await readJSON(request);
    let result;
    if (collection === "customers")
      result = await writeCustomer(actor, id, body);
    else if (collection === "projects")
      result = await writeProject(actor, id, body);
    else return methodNotAllowed("GET");
    return json(result, id ? 200 : 201);
  } catch (error) {
    if (error instanceof z.ZodError)
      return json(
        {
          error: "Invalid input",
          issues: error.issues.map(({ path, message }) => ({ path, message })),
        },
        400,
      );
    if (error instanceof APIError && error.isPublic)
      return json({ error: error.message }, error.status);
    return json(
      { error: "The operation failed. No change was confirmed." },
      500,
    );
  }
}
