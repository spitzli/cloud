import { createHostingBindingRuntime } from "@webdock/module-hosting/server/binding-runtime";
import { bindingMailReady } from "./mail-binding";
import { currentDatabaseScope } from "@webdock/database-runtime";
import { decryptMailSecret } from "../platform/platform";
export type { BindingMaterial } from "@webdock/module-hosting/server/binding-runtime";
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
