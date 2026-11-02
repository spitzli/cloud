import { createMailSending } from "@webdock/module-mail/server/product-sending";
import { mailDatabase as database } from "./store";
import { withMailTransaction } from "./store";
import { assertMailAccess } from "./access";
import { assertVerifiedSender } from "./domains";
import { runIdempotent } from "./idempotency";
import { consumeMailRate } from "./rate-limit";
import { getSuppressedRecipients } from "./suppressions";
import { reserveMailQuota, settleMailQuota } from "./quota";
import { recordMailEvent } from "./events";
import { openMailAttachments, sealMailAttachments } from "./content";
import { resolveSendingCredential } from "./provider-credentials";
import { sendTurboMail } from "./turbo-send";
export const { submitMail, dispatchMail, persistMailMessage } =
  createMailSending({
    database,
    withMailTransaction,
    assertMailAccess,
    assertVerifiedSender,
    runIdempotent,
    consumeMailRate,
    getSuppressedRecipients,
    reserveMailQuota,
    settleMailQuota,
    recordMailEvent,
    openMailAttachments,
    sealMailAttachments,
    resolveSendingCredential,
    sendTurboMail,
  });
