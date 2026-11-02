import { TurboDomainsClient, senderDomain } from "./turbo-domains";
import { createTenantDomains } from "@webdock/module-mail/server/tenant-domains";
import { resolveSessionReference } from "../auth/principal";
import { requireMail } from "../../installation";
import { withAuthenticatedDatabase } from "../../db/database-context";
import { database } from "../../db/runtime";
import { getTenant } from "../access/tenants";
import { getPlatformSettings, mailProvider } from "../platform/platform";
export {
  MailDomainsError,
  senderDomainSchemaSQL,
  type TenantSenderDomain,
} from "@webdock/module-mail/server/tenant-domains";
export const { getTenantSenderDomains, manageTenantSenderDomain } =
  createTenantDomains({
    resolveSessionReference,
    requireMail,
    withAuthenticatedDatabase,
    database,
    getTenant,
    getPlatformSettings,
    mailProvider,
    domainProvider: (auth) => new TurboDomainsClient(auth),
    senderDomain,
  });
