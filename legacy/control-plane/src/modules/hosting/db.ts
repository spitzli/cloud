import { createHostingDb } from "@webdock/module-hosting/server/db";
import { database } from "../../db/runtime";
export const { transaction, audit, planLock, clusterLock } = createHostingDb({
  get database() {
    return database;
  },
});
