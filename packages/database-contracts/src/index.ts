import { z } from "zod";
import { resourceID } from "@spitzli/hosting-contracts";

export const databaseEngine = z.enum(["postgresql", "sqlite"]);
export const accessProfile = z.enum(["read", "write", "schema"]);
export type AccessProfile = z.infer<typeof accessProfile>;
export const runtimeOrigin = z.string().max(300).transform((value, ctx) => {
  try {
    const url = new URL(value);
    const local = url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
    if ((!local && url.protocol !== "https:") || url.username || url.password || url.pathname !== "/" || url.search || url.hash) throw Error();
    return url.origin;
  } catch {
    ctx.addIssue({ code: "custom", message: "Use an HTTPS origin without credentials or a path." });
    return z.NEVER;
  }
});
export const databaseCommand = z.discriminatedUnion("action", [
  z.object({ action: z.literal("list"), customerID: resourceID }).strict(),
  z.object({ action: z.literal("get"), bindingID: resourceID }).strict(),
  z.object({ action: z.literal("create"), engine: databaseEngine.default("postgresql"), projectID: resourceID, name: z.string().trim().min(1).max(120), environment: z.enum(["production", "staging", "development"]) }).strict(),
  z.object({ action: z.literal("grant"), bindingID: resourceID, subject: z.string().min(1).max(128), profile: accessProfile,
    runtimeOrigin, connectionID: z.string().min(1).max(128), proxySecret: z.string().min(32).max(512),
    isolationVerified: z.literal(true), databaseRoleVerified: z.literal(true),
  }).strict(),
  z.object({ action: z.literal("revoke"), bindingID: resourceID, subject: z.string().min(1).max(128) }).strict(),
  z.object({ action: z.literal("disable"), bindingID: resourceID }).strict(),
  z.object({ action: z.literal("open"), bindingID: resourceID }).strict(),
  z.object({ action: z.literal("openManaged"), databaseID: resourceID }).strict(),
]);
export type DatabaseCommand = z.infer<typeof databaseCommand>;
export const publicBinding = z.object({
  id: resourceID, customerID: resourceID, projectID: resourceID, name: z.string(),
  environment: z.enum(["production", "staging", "development"]), engine: databaseEngine,
  profile: accessProfile.nullable(),
});
export type DatabaseBinding = z.infer<typeof publicBinding>;
export type ManagedDatabaseEntry = { id: string; projectID: string; name: string; state: string; canOpen: boolean };
export type DatabaseDirectory = { bindings: DatabaseBinding[]; managed: ManagedDatabaseEntry[]; operator: boolean; projects: { id: string; name: string }[]; people: { id: string; name: string; email: string }[] };
export type DatabaseLaunch = { code: string; gatewayOrigin: string; binding: DatabaseBinding; expiresAt: string };
/** Confidential gateway contract: never serialize to Studio/browser responses. */
export type RuntimeAccess = {
  scopeID: string; subject: string; connectionID: string; profile: AccessProfile;
  runtimeOrigin: string; proxySecret: string; expiresAt: string;
};
/** Confidential gateway-only credentials. Never send this object to a browser. */
export type ManagedRuntimeAccess = {
  kind: "managed"; scopeID: string; subject: string; connectionID: string;
  profile: "schema"; expiresAt: string; revision: string;
  database: { host: string; port: number; database: string; username: string; password: string; caCertificate: string };
};
export type GatewayAccess = RuntimeAccess | ManagedRuntimeAccess;
