import { readAuthProviderConfiguration } from "./index";
type Environment = Record<string, string | undefined>;
export function readAuthProtocolSelection(env: Environment) {
  const browserProvider = readAuthProviderConfiguration(env.SPITZLI_AUTH_PROVIDER).provider;
  const oauthProvider = env.SPITZLI_OAUTH_PROVIDER ??
    (browserProvider === "better-auth" ? "better-auth" : "disabled");
  if (oauthProvider !== "better-auth" && oauthProvider !== "disabled")
    throw Error("Unsupported OAuth provider; select better-auth or disabled");
  if (browserProvider === "better-auth" && oauthProvider === "disabled")
    throw Error("Better Auth browser authentication currently includes its OAuth capability");
  return { browserProvider, oauthProvider, compatibility: browserProvider === "clerk" && oauthProvider === "better-auth" } as const;
}
export type AuthProtocolSelection = ReturnType<typeof readAuthProtocolSelection>;
export function requireOAuthCapability(env: Environment = process.env) {
  if (readAuthProtocolSelection(env).oauthProvider !== "better-auth")
    throw Error("Legacy OAuth capability is disabled");
}
