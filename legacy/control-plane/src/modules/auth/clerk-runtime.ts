import { requireOAuthCapability } from "@webdock/auth-configuration/protocols";
import { selectAuthPlugin } from "@webdock/auth-configuration/selection";
import { betterAuthIdentity } from "@webdock/auth-better-auth";
import { signCompatibilitySession } from "@webdock/auth-better-auth/compatibility";
import { authDatabase, workerDatabase } from "../../db/runtime";
import { createClerkMappings } from "./clerk-mappings";
import { createClerkSessionBridge } from "./clerk-bridge";

const selected = selectAuthPlugin(process.env.WEBDOCK_AUTH_PROVIDER);
const configuration = selected.configuration;
export const browserAuthProvider = configuration.provider;
export async function getClerkBrowserAdapter() {
  if (configuration.provider !== "clerk")
    throw Error("Clerk is not the selected browser provider");
  const { createPlugin } = await import("@webdock/auth-clerk/plugin");
  const { createClerkBackend } = await import("@webdock/auth-clerk/sdk");
  return createPlugin({
    config: configuration,
    backend: createClerkBackend(
      process.env.CLERK_SECRET_KEY ?? "",
      process.env.CLERK_JWT_KEY,
    ),
    mappings: createClerkMappings(
      process.env.WEBDOCK_PROCESS_ROLE === "worker"
        ? workerDatabase
        : authDatabase,
    ),
    local: betterAuthIdentity,
  });
}
export async function validateBrowserAuthConfiguration() {
  await selected.load();
  if (configuration.provider === "clerk") {
    if ((process.env.WEBDOCK_CONSOLE_SERVICE_KEY?.length ?? 0) < 32)
      throw Error("Clerk Console requires WEBDOCK_CONSOLE_SERVICE_KEY with at least 32 characters");
    if (!/^sk_(live|test)_/.test(process.env.CLERK_SECRET_KEY ?? ""))
      throw Error("Configured Clerk backend secret required");
    const origin = new URL(
      process.env.WEBDOCK_CONSOLE_ORIGIN ??
        process.env.WEBDOCK_SSO_APP_ORIGIN ??
        "",
    ).origin;
    if (!configuration.authorizedParties.includes(origin))
      throw Error("Studio origin must be a configured Clerk authorized party");
  }
  return browserAuthProvider;
}
export async function clerkBrowserSession(headers: Headers) {
  requireOAuthCapability();
  const clerk = await getClerkBrowserAdapter();
  return createClerkSessionBridge({
    database: authDatabase,
    verifyExternal: clerk.verifyExternal,
    origin: new URL(process.env.BETTER_AUTH_URL ?? "http://localhost:3125")
      .origin,
    async signSession(token) {
      return signCompatibilitySession(
        process.env.BETTER_AUTH_URL ?? "http://localhost:3125",
        process.env.BETTER_AUTH_SECRET ?? "",
        token,
      );
    },
  })(headers);
}
