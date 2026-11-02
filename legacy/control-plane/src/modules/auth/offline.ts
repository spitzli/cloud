import { AsyncLocalStorage } from "node:async_hooks";
// Scoped to explicit offline CLI calls, never derived from HTTP headers/body.
export const offlineProvisioning = new AsyncLocalStorage<{
  clientID?: string;
}>();
