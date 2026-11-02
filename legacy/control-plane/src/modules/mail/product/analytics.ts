import { createMailAnalytics } from "@webdock/module-mail/server/product-analytics";
import { mailDatabase as database } from "./store";
import { assertMailAccess } from "./access";
export const { getMailAnalytics } = createMailAnalytics({
  database,
  assertMailAccess,
});
