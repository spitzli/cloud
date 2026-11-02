import { resolveSessionReference } from "../auth/principal";
import { withAuthenticatedDatabase } from "../../db/database-context";
import { authorizeQueuedMailbox, authorizeMailbox } from "./mail-access";
import { transaction } from "../hosting/db";
import { createNativeSessions } from "@webdock/module-mail/server/native-sessions";
export const {
  launchWebmail,
  exchangeWebmail,
  inspectWebmail,
  inspectQueuedWebmail,
  revokeWebmail,
  launchTenantWebmail,
} = createNativeSessions({
  resolveSessionReference,
  withAuthenticatedDatabase,
  authorizeQueuedMailbox,
  authorizeMailbox,
  env: process.env,
  transaction,
});
