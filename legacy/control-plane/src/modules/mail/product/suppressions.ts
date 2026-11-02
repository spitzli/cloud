import { createMailSuppressions } from "@webdock/module-mail/server/product-suppressions";
import { mailDatabase as database, withMailTransaction } from "./store";
import { assertMailAccess } from "./access";
export const {
  getSuppressedRecipients,
  setMailSuppression,
  removeMailSuppression,
  listMailSuppressions,
} = createMailSuppressions({ database, withMailTransaction, assertMailAccess });
