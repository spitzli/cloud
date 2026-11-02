import { createMailHealth } from "@webdock/module-mail/server/product-health";
import { mailDatabase as database } from "./store";
import { assertMailAccess } from "./access";
export const { getMailHealth } = createMailHealth({
  database,
  assertMailAccess,
});
