import { readAuthProtocolSelection, type AuthProtocolSelection } from "@webdock/auth-configuration/protocols";
import type { ApiConfig } from "../config";
import type { Mount } from "../route-manifest";
export { readAuthProtocolSelection, type AuthProtocolSelection } from "@webdock/auth-configuration/protocols";
/** Better Auth remains the existing OAuth issuer only when explicitly selected. */
export async function createAuthProtocolMounts(
  config: ApiConfig,
  selection: AuthProtocolSelection = readAuthProtocolSelection(process.env),
  load = async (): Promise<Mount[]> => {
    const { auth } = await import("../modules/auth/auth");
    const { authProtocol } = await import("./auth");
    const { authUIProtocol } = await import("./auth-ui");
    await auth.$context;
    return [authProtocol(config, request => auth.handler(request)), authUIProtocol(config)];
  },
): Promise<Mount[]> {
  if (config.domains.includes("mcp") && selection.oauthProvider === "disabled")
    throw Error("MCP requires an explicitly selected OAuth provider (WEBDOCK_OAUTH_PROVIDER=better-auth)");
  if (!config.domains.includes("auth")) return [];
  if (selection.browserProvider === "clerk" && selection.oauthProvider === "disabled") return [];
  if ((process.env.BETTER_AUTH_SECRET?.length ?? 0) < 32)
    throw Error("Authentication secret required");
  return load();
}
