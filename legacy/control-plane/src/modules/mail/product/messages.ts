import { createMailMessages } from "@webdock/module-mail/server/product-messages";
import { mailDatabase as database } from "./store";
import { assertMailAccess } from "./access";
export { type MessageQuery } from "@webdock/module-mail/server/product-messages";
export const { listMailMessages, getMailMessage } = createMailMessages({
  database,
  assertMailAccess,
});
