import { createHostingDatabaseObservations } from "@webdock/module-databases/server/database-observations";
import { verifyAgent } from "./clusters";
import { transaction } from "./db";
export const { databaseObservationConfig, reportDatabaseObservations } =
  createHostingDatabaseObservations({
    get verifyAgent() {
      return verifyAgent;
    },
    get transaction() {
      return transaction;
    },
  });
