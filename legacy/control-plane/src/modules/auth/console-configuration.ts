import { readAuthProviderConfiguration } from "@webdock/auth-configuration";
export function consoleAuthConfiguration(
  env: Record<string, string | undefined> = process.env,
) {
  const configured = readAuthProviderConfiguration(env.WEBDOCK_AUTH_PROVIDER);
  const clerk = configured.provider === "clerk" ? configured : undefined;
  if (clerk && env.CLERK_ISSUER && env.CLERK_ISSUER !== clerk.issuer)
    throw Error("Configured Clerk issuers must match");
  return {
    // Selected Clerk uses server delegation only after explicit service-key provisioning.
    enabled: clerk ? !!env.WEBDOCK_CONSOLE_SERVICE_KEY : env.WEBDOCK_CONSOLE_AUTH_PROVIDER === "clerk",
    issuer: clerk?.issuer ?? env.CLERK_ISSUER,
    authorizedParties:
      clerk?.authorizedParties ??
      (env.WEBDOCK_CONSOLE_ORIGIN ? [env.WEBDOCK_CONSOLE_ORIGIN] : []),
    audience: clerk?.audience,
    requireExistingMapping: !!clerk,
    serviceKey: env.WEBDOCK_CONSOLE_SERVICE_KEY,
  };
}
