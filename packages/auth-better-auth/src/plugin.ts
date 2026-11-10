import type { TrustedAuthPlugin } from "@spitzli/core/auth-plugin";
import { betterAuthIdentity, createBetterAuthAuthenticator } from "./index";
export const manifest = { id: "auth-better-auth", kind: "authentication" } as const;
export function createPlugin(resolve: Parameters<typeof createBetterAuthAuthenticator>[0]): TrustedAuthPlugin {
  return { manifest, identity: betterAuthIdentity, authenticator: createBetterAuthAuthenticator(resolve) };
}
