import { createMailCallbackInbox } from "@spitzli/module-mail/server/product-callback-inbox";
import { authDatabase } from "../../../db/runtime";
import { withDatabaseScope } from "@spitzli/database-runtime";
import { discardExpiredUncertainContent } from "./content";
import { mailDatabase as database } from "./store";
import { withMailTransaction } from "./store";
import { readBoundedText } from "./http";
import { canonicalHash, refreshBatchIdempotency } from "./idempotency";
import { recordMailEvent } from "./events";
import { settleMailQuota } from "./quota";
export { type NormalizedProviderEvent } from "@spitzli/module-mail/server/product-callback-inbox";
export const {
  parseProviderEvents,
  acceptProviderEvents,
  applyProviderEvent,
  processCallbackInbox,
} = createMailCallbackInbox({
  authDatabase,
  withDatabaseScope,
  discardExpiredUncertainContent,
  database,
  withMailTransaction,
  readBoundedText,
  canonicalHash,
  refreshBatchIdempotency,
  recordMailEvent,
  settleMailQuota,
});
