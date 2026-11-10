import { createRegistryStore } from "@spitzli/core/registry/store";
import { nextRegistryID as allocateRegistryID } from "@spitzli/core/registry/snowflake";
import type { RegistryIdentity } from "@spitzli/core/registry/ports";
import { registryService } from "./context";
// Historical maintenance tools retain DTO typing; runtime CMS queries fail closed.
type HistoricalRecords = {
  "cms-instances": import("@spitzli/api-client/registry").CmsInstance;
};
export type RegistryStore =
  import("@spitzli/core/registry/store").RegistryStore<HistoricalRecords>;
export async function registryStore(identity: RegistryIdentity) {
  return createRegistryStore<HistoricalRecords>(
    await registryService(),
    identity,
  );
}
export async function nextRegistryID() {
  return allocateRegistryID((await registryService()).raw);
}
