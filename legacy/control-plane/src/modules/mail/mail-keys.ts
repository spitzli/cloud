import { TurboSMTPClient } from "./turbosmtp";
import { createTenantKeys } from "@webdock/module-mail/server/tenant-keys";
import { resolveSessionReference } from "../auth/principal";
import { withAuthenticatedDatabase } from "../../db/database-context";
import { database } from "../../db/runtime";
import { getTenant } from "../access/tenants";
import { getPlatformSettings, mailProvider } from "../platform/platform";
export {
  MailKeysError,
  type MailKeyInput,
} from "@webdock/module-mail/server/tenant-keys";
export const { getTenantMailKeys, manageTenantMailKeys } = createTenantKeys({
  resolveSessionReference,
  withAuthenticatedDatabase,
  database,
  getTenant,
  getPlatformSettings,
  mailProvider,
  childProvider: (auth) => new TurboSMTPClient(auth),
});
