import { createTenantMail } from "@webdock/module-mail/server/tenant-mail";
import { resolveSessionReference } from "../auth/principal";
import { requireMail } from "../../installation";
import { withAuthenticatedDatabase } from "../../db/database-context";
import { database } from "../../db/runtime";
import { getTenant } from "../access/tenants";
import {
  encryptMailSecret,
  getMailConnectionStatus,
  getPlatformSettings,
  mailProvider,
} from "../platform/platform";
import { TurboSMTPError } from "./turbosmtp";
export {
  TenantMailError,
  mailSchemaSQL,
  normalizeMailDomain,
  type MailView,
} from "@webdock/module-mail/server/tenant-mail";
const runtime = createTenantMail({
  database,
  withAuthenticatedDatabase,
  getTenant,
  resolveSessionReference,
  encryptMailSecret,
  getMailConnectionStatus,
  getPlatformSettings,
  requireMail,
  provider: (customerID) => mailProvider(undefined, customerID),
  definitelyRejectedCreation: (error) =>
    error instanceof TurboSMTPError &&
    error.code === "upstream" &&
    [400, 401, 403].includes(error.status || 0),
});
export const { getTenantMail, manageTenantMail } = runtime;
