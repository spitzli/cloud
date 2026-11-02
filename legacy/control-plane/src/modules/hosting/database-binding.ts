import { requireDatabases } from "./product-admission";
import { createHostingDatabaseBinding } from "@webdock/module-databases/server/database-binding";
import { decryptMailSecret } from "../platform/platform";
export const { databaseConnectionURL, databaseBindingMaterial } =
  createHostingDatabaseBinding({
    requireDatabases,
    get decryptSecret() {
      return decryptMailSecret;
    },
  });
