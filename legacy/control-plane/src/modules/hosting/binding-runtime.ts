import { createHostingBindingRuntime } from "@spitzli/module-hosting/server/binding-runtime";
import { bindingMailReady } from "./mail-binding";
import { currentDatabaseScope } from "@spitzli/database-runtime";
import { decryptMailSecret } from "../platform/platform";
export type { BindingMaterial } from "@spitzli/module-hosting/server/binding-runtime";
export const { openBinding, bindingRuntime } = createHostingBindingRuntime({
  get bindingMailReady() {
    return bindingMailReady;
  },
  get currentDatabaseScope() {
    return currentDatabaseScope;
  },
  get decryptMailSecret() {
    return decryptMailSecret;
  },
  env: process.env,
});
