import { createMailRateLimit } from "@webdock/module-mail/server/product-rate-limit";
import { withMailTransaction } from "./store";
import { assertMailAccess } from "./access";
export const { consumeMailRate } = createMailRateLimit({
  withMailTransaction,
  assertMailAccess,
});
