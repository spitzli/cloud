import { z } from "zod";
import {
  publicBinding,
  runtimeOrigin,
  accessProfile,
  type AccessProfile,
} from "@spitzli/database-contracts";
import type { DatabaseWorkspaceTarget } from "@spitzli/module-databases/providers";
const launchSchema = z
  .object({
    code: z.string().regex(/^[A-Za-z0-9_-]{43}$/),
    gatewayOrigin: runtimeOrigin,
    binding: publicBinding,
    expiresAt: z.iso.datetime(),
  })
  .strict();
/** UI hints only. The gateway policy and bound PostgreSQL role remain authoritative. */
export function tabularisCapabilities(
  target: DatabaseWorkspaceTarget,
  profile: AccessProfile = "read",
) {
  accessProfile.parse(profile);
  if (target.kind === "platform" && profile !== "read")
    throw Error("Platform database inspection is read-only");
  return {
    readonly: profile === "read",
    writeRows: profile !== "read",
    manageSchema: profile === "schema",
    manageUsers: false,
    configureConnections: false,
  } as const;
}
/** Browser-safe launch data. Raw database endpoints and credentials are never accepted. */
export function resolveTabularisLaunch(
  input: unknown,
  target: DatabaseWorkspaceTarget,
  now = Date.now(),
) {
  if (target.kind === "platform")
    throw Error(
      "Platform database launch requires a separately authorized read-only endpoint",
    );
  const launch = launchSchema.parse(input);
  const id =
    target.kind === "managed-customer" ? target.databaseID : target.bindingID;
  if (
    launch.binding.id !== id ||
    launch.binding.customerID !== target.customerID ||
    launch.binding.projectID !== target.projectID ||
    !launch.binding.profile ||
    Date.parse(launch.expiresAt) <= now
  )
    throw Error("Database launch target or lifetime does not match");
  return {
    ...launch,
    capabilities: tabularisCapabilities(target, launch.binding.profile),
  };
}
export function tabularisSessionEndpoint(
  input: unknown,
  gatewayOrigin: string,
) {
  const session = z
    .object({
      scopeID: z.string().regex(/^[A-Za-z0-9_-]+$/),
      connectionID: z.string().min(1).max(128),
      baseUrl: z.string(),
    })
    .passthrough()
    .parse(input);
  const origin = runtimeOrigin.parse(gatewayOrigin);
  if (session.baseUrl !== `${origin}/s/${session.scopeID}`)
    throw Error("Database session endpoint does not match");
  return {
    scopeID: session.scopeID,
    connectionID: session.connectionID,
    baseUrl: session.baseUrl,
  };
}
