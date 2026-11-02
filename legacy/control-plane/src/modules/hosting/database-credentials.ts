import { createHostingDatabaseCredentials } from "@webdock/module-databases/server/database-credentials";
import { withHostingDatabase } from "../../db/database-context";
import { transaction, audit } from "./db";
import { authorizeHosting } from "./authorization";
import { databaseAccess } from "./databases";
import { decryptMailSecret } from "../platform/platform";
export const { displayDatabaseCredentials } = createHostingDatabaseCredentials({
  get withHostingDatabase() {
    return withHostingDatabase;
  },
  get transaction() {
    return transaction;
  },
  get audit() {
    return audit;
  },
  get authorizeHosting() {
    return authorizeHosting;
  },
  get databaseAccess() {
    return databaseAccess;
  },
  get decryptSecret() {
    return decryptMailSecret;
  },
});
