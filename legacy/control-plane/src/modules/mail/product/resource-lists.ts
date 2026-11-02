import { createMailResourceLists } from "@webdock/module-mail/server/product-resource-lists";
import { mailDatabase as database } from "./store";
import { assertMailAccess } from "./access";
export type { MailListKind } from "@webdock/module-mail/server/product-resource-lists";
export const { listMailResource, getMailLog } = createMailResourceLists({
  database,
  assertMailAccess,
});
