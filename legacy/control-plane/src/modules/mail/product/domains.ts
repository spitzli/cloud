import { createMailDomains } from "@webdock/module-mail/server/product-domains";
import { requireMail } from "../../../installation";
import { mailFetch } from "./deadline";
import { mailDatabase as database, withMailTransaction } from "./store";
import { mailProvider } from "../../platform/platform";
import { TurboDomainsClient, senderDomain } from "../turbo-domains";
import { assertMailAccess } from "./access";
import { resolveSendingCredential } from "./provider-credentials";
export const {
  assertVerifiedSender,
  claimMailDomain,
  removeMailDomain,
  listMailDomains,
  verifyMailDomain,
  getMailDomain,
} = createMailDomains({
  database,
  withMailTransaction,
  assertMailAccess,
  requireMail,
  mailFetch,
  senderDomain,
  resolveSendingCredential,
  master: (actor, fetcher) =>
    mailProvider(fetcher, actor.customerID, actor.connectionID),
  domainProvider: (auth, fetcher) => new TurboDomainsClient(auth, fetcher),
});
