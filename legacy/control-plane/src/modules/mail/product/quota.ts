import { createMailQuota } from "@webdock/module-mail/server/product-quota";
import { mailDatabase as database, withMailTransaction } from "./store";
import { assertMailAccess } from "./access";
export const {
  reserveMailQuota,
  settleMailQuota,
  assertQuotaCapacity,
  recoverTerminalQuotas,
} = createMailQuota({ database, withMailTransaction, assertMailAccess });
