import { createMailIdempotency } from "@webdock/module-mail/server/product-idempotency";
import { mailDatabase as database, withMailTransaction } from "./store";
import { assertMailAccess } from "./access";
export const {
  canonicalHash,
  reserveMailRequest,
  completeMailRequest,
  runIdempotent,
  refreshBatchIdempotency,
} = createMailIdempotency({ database, withMailTransaction, assertMailAccess });
