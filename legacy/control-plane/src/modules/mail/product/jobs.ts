import { createMailJobs } from "@webdock/module-mail/server/product-jobs";
import { requireMail } from "../../../installation";
import { recoverTerminalQuotas } from "./quota";
import { refreshBatchIdempotency } from "./idempotency";
import { mailDatabase as database } from "./store";
import { withMailTransaction } from "./store";
import { dispatchMail } from "./sending";
import { advanceMailBatch } from "./batches";
import { processCallbackInbox } from "./callback-inbox";
import { deliverDueWebhooks } from "./webhooks";
import { pruneMailData } from "./retention";
import { recordMailEvent } from "./events";
export { mailJobs, type MailJob } from "@webdock/module-mail/server/product-jobs";
export const { runMailJob } = createMailJobs({
  requireMail,
  recoverTerminalQuotas,
  refreshBatchIdempotency,
  database,
  withMailTransaction,
  dispatchMail,
  advanceMailBatch,
  processCallbackInbox,
  deliverDueWebhooks,
  pruneMailData,
  recordMailEvent,
});
