import { createMailReconcile } from "@webdock/module-mail/server/product-reconcile";
import { discardExpiredUncertainContent } from "./content";
import { mailDatabase as database } from "./store";
import { withMailTransaction } from "./store";
import { assertMailAccess } from "./access";
import { mailManagementHeaders } from "./provider-credentials";
import { recordMailEvent } from "./events";
import { settleMailQuota } from "./quota";
import { canonicalHash, refreshBatchIdempotency } from "./idempotency";
export const { reconcileMail } = createMailReconcile({
  discardExpiredUncertainContent,
  database,
  withMailTransaction,
  assertMailAccess,
  mailManagementHeaders,
  recordMailEvent,
  settleMailQuota,
  canonicalHash,
  refreshBatchIdempotency,
});
