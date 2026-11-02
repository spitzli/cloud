import { requireHostingCommand } from "../../installation";
import { RegistryError as APIError } from "../registry/service";
import {
  commandSchema,
  HostingError,
  hostingError,
  type HostingCommand,
} from "@webdock/hosting-contracts";
import { readJSON } from "./registry-api";
export async function handleHostingRequest(
  request: Request,
  path: string[],
  deps: {
    origin: string;
    call: (token: string, cmd: HostingCommand) => Promise<unknown>;
  },
) {
  try {
    const origin = request.headers.get("origin");
    if (origin && origin !== deps.origin)
      throw new HostingError(403, "Invalid origin.");
    const token = request.headers
      .get("authorization")
      ?.match(/^Bearer (\S+)$/)?.[1];
    if (!token || token.length > 16384)
      throw new HostingError(401, "Hosting authentication required.");
    let command: Record<string, unknown> | undefined;
    const [area, id, child] = path,
      method = request.method;
    if (method === "POST" && path.length === 1 && area === "commands") {
      const parsed = commandSchema.parse(await readJSON(request));
      if (
        !parsed.action.startsWith("byok.") &&
        !parsed.action.startsWith("git.") &&
        !parsed.action.startsWith("databases.")
      )
        throw new HostingError(
          400,
          "Use the resource endpoint for this action.",
        );
      requireHostingCommand(parsed.action);
      const data = await deps.call(token, parsed);
      return Response.json(data, { headers: { "Cache-Control": "no-store" } });
    }
    if (method === "GET") {
      const params = new URL(request.url).searchParams;
      const query: Record<string, unknown> = {};
      for (const [k, v] of params) {
        if (
          !["page", "limit", "search", "sort"].includes(k) ||
          params.getAll(k).length !== 1
        )
          throw new HostingError(400, "Invalid query.");
        query[k] =
          k === "search" || k === "sort"
            ? v
            : /^[0-9]+$/.test(v)
              ? Number(v)
              : NaN;
      }
      if (area === "apps" && path.length === 2)
        command = { action: "apps.get", appID: id, ...query };
      else if (
        area === "apps" &&
        path.length === 3 &&
        child === "storage-deletion"
      )
        command = { action: "apps.storageDeletion", appID: id, ...query };
      else if (area === "apps" && path.length === 3 && child === "deletion")
        command = { action: "apps.deletion", appID: id, ...query };
      else if (area === "projects" && path.length === 3 && child === "apps")
        command = { action: "apps.list", projectID: id, ...query };
      else if (area === "clusters" && path.length === 1)
        command = { action: "clusters.list", ...query };
      else if (area === "clusters" && path.length === 2)
        command = { action: "clusters.get", clusterID: id, ...query };
      else if (
        area === "customers" &&
        path.length === 3 &&
        ["limits", "usage", "projects"].includes(child)
      )
        command = {
          action: child === "projects" ? "projects.list" : child + ".get",
          customerID: id,
          ...query,
        };
      else if (area === "operations" && path.length === 2)
        command = { action: "operations.get", operationID: id, ...query };
    } else {
      const body = await readJSON(request);
      if (!body || typeof body !== "object" || Array.isArray(body))
        throw new HostingError(400, "Invalid hosting input.");
      if (
        Object.keys(body).some(
          (k) =>
            [
              "action",
              "customerID",
              "clusterID",
              "projectID",
              "appID",
            ].includes(k) &&
            !(area === "projects" && method === "POST" && k === "clusterID") &&
            !(area === "customers" && child === "limits" && k === "projectID"),
        )
      )
        throw new HostingError(400, "Target must come from the request path.");
      const data = body as Record<string, unknown>;
      if (
        area === "projects" &&
        path.length === 3 &&
        child === "apps" &&
        method === "POST"
      )
        command = { ...data, action: "apps.create", projectID: id };
      else if (area === "apps" && path.length === 2 && method === "PATCH")
        command = { ...data, action: "apps.update", appID: id };
      else if (
        area === "apps" &&
        path.length === 3 &&
        child === "storage" &&
        method === "DELETE"
      )
        command = { ...data, action: "apps.purgeStorage", appID: id };
      else if (area === "apps" && path.length === 2 && method === "DELETE")
        command = { ...data, action: "apps.delete", appID: id };
      else if (
        area === "apps" &&
        path.length === 3 &&
        method === "POST" &&
        [
          "start",
          "stop",
          "scale",
          "restart",
          "rollback",
          "logs",
          "reconcile",
        ].includes(child)
      )
        command = { ...data, action: "apps." + child, appID: id };
      else if (area === "clusters" && path.length === 1 && method === "POST")
        command = { ...data, action: "clusters.register" };
      else if (
        area === "clusters" &&
        path.length === 3 &&
        method === "POST" &&
        ["enrollment", "revoke", "activate"].includes(child)
      )
        command = { ...data, action: "clusters." + child, clusterID: id };
      else if (
        area === "customers" &&
        child === "limits" &&
        path.length === 3 &&
        method === "PUT"
      )
        command = { ...data, action: "limits.set", customerID: id };
      else if (
        area === "projects" &&
        path.length === 2 &&
        ["POST", "PATCH"].includes(method)
      )
        command = {
          ...data,
          action: method === "POST" ? "projects.create" : "projects.update",
          projectID: id,
        };
    }
    if (!command)
      throw new HostingError(404, "Hosting endpoint is unavailable.");
    const parsed = commandSchema.parse(command);
    requireHostingCommand(parsed.action);
    const data = await deps.call(token, parsed);
    return Response.json(data, {
      status:
        data &&
        typeof data === "object" &&
        "status" in data &&
        data.status === "queued"
          ? 202
          : 200,
      headers: { "Cache-Control": "no-store" },
    });
  } catch (error) {
    const safe =
      error instanceof APIError && error.status >= 400 && error.status < 500
        ? { status: error.status, message: "Invalid hosting request." }
        : hostingError(error);
    return Response.json(
      { error: safe.message },
      { status: safe.status, headers: { "Cache-Control": "no-store" } },
    );
  }
}
