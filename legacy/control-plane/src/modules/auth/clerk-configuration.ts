import { readAuthProviderConfiguration } from "@webdock/auth-configuration";
/** Workers verify accepted user authority with their restricted database role, never bootstrap credentials. */
export function validateClerkWorkerConfiguration(
  domains: readonly string[],
  env: Record<string, string | undefined> = process.env,
): void {
  const config = readAuthProviderConfiguration(env.WEBDOCK_AUTH_PROVIDER);
  if (
    domains.some((domain) => domain === "mail" || domain === "hosting") &&
    config.provider === "clerk" &&
    !/^sk_(live|test)_/.test(env.CLERK_SECRET_KEY ?? "")
  )
    throw Error(
      "Clerk Mail/Hosting workers require a backend verification key",
    );
}
