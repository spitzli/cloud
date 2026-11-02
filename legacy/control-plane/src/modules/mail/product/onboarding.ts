import { createMailOnboarding } from "@webdock/module-mail/server/product-onboarding";
import { resolveSessionReference } from "../../auth/principal";
import { requireMail } from "../../../installation";
import { withAuthenticatedDatabase } from "../../../db/database-context";
import { TurboDomainsClient } from "../turbo-domains";
import { mailDatabase as database, withMailTransaction } from "./store";
import { getTenant } from "../../access/tenants";
import { currentTenantPreview } from "../../access/tenant-preview";
import { byokPolicy } from "../../hosting/byok";
import { encryptMailSecret } from "../../platform/platform";
import { assertMailManager } from "./access";
import { verifyTurboSendingAccount } from "./provider-account";
import { ensureManagedSendingCredential } from "./provider-credentials";
export const {
  listTenantMailConnections,
  connectByokMail,
  getMailOnboarding,
  configureManagedMail,
} = createMailOnboarding({
  database,
  withMailTransaction,
  getTenant,
  resolveSessionReference,
  withAuthenticatedDatabase,
  requireMail,
  encryptMailSecret,
  currentTenantPreview,
  byokPolicy,
  assertMailManager,
  verifyTurboSendingAccount,
  ensureManagedSendingCredential,
  listProviderDomains: (pair) =>
    new TurboDomainsClient(pair).listSenderDomains(),
});
