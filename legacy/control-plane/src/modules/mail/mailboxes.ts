import { clerkMailAccess, clerkMailPeople } from "./mail-clerk-access";
import { resolveSessionReference } from "../auth/principal";
import { requireMail } from "../../installation";
import { withAuthenticatedDatabase } from "../../db/database-context";
import { authorizeMail } from "./mail-access";
import { transaction } from "../hosting/db";
import { getTenant } from "../access/tenants";
import { createNativeMailboxes } from "@webdock/module-mail/server/native-mailboxes";
export const {
  queueMailConfiguration,
  executeMailboxes,
  getTenantMailboxes,
  manageTenantMailboxes,
} = createNativeMailboxes({
  clerkMailAccess,
  clerkMailPeople,
  resolveSessionReference,
  requireMail,
  withAuthenticatedDatabase,
  authorizeMail,
  env: process.env,
  transaction,
  getTenant,
});
