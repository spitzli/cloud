import { createMailBatches } from "@webdock/module-mail/server/product-batches";
import { mailDatabase as database } from "./store";
import { withMailTransaction } from "./store";
import { assertMailAccess } from "./access";
import { assertVerifiedSender } from "./domains";
import { runIdempotent, refreshBatchIdempotency } from "./idempotency";
import { consumeMailRate } from "./rate-limit";
import { persistMailMessage, dispatchMail } from "./sending";
import { sendTurboMail } from "./turbo-send";
import { getSuppressedRecipients } from "./suppressions";
import { assertQuotaCapacity } from "./quota";
export const { getMailBatch, submitMailBatch, advanceMailBatch } =
  createMailBatches({
    database,
    withMailTransaction,
    assertMailAccess,
    assertVerifiedSender,
    runIdempotent,
    refreshBatchIdempotency,
    consumeMailRate,
    persistMailMessage,
    dispatchMail,
    sendTurboMail,
    getSuppressedRecipients,
    assertQuotaCapacity,
  });
