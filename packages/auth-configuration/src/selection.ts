import { lazyAuthPlugin } from "@spitzli/core/auth-plugin";
import { readAuthProviderConfiguration } from "./index";
/** Only the chosen trusted provider entry is imported. No provider runtime is constructed here. */
export function selectAuthPlugin(raw: string | undefined) {
  const configuration = readAuthProviderConfiguration(raw);
  return {
    configuration,
    load: configuration.provider === "clerk"
      ? lazyAuthPlugin(() => import("@spitzli/auth-clerk/plugin"))
      : lazyAuthPlugin(() => import("@spitzli/auth-better-auth/plugin")),
  };
}
