import { createMailWebhooks } from "@webdock/module-mail/server/product-webhooks";
import { mailDatabase as database } from "./store";
import { encryptMailSecret, decryptMailSecret } from "../../platform/platform";
import { withMailTransaction } from "./store";
import { assertMailAccess } from "./access";
export const {
  createMailWebhook,
  listMailWebhooks,
  getMailWebhook,
  updateMailWebhook,
  deleteMailWebhook,
  rotateMailWebhook,
  replayMailWebhook,
  listMailWebhookDeliveries,
  deliverDueWebhooks,
} = createMailWebhooks({
  database,
  encryptMailSecret,
  decryptMailSecret,
  withMailTransaction,
  assertMailAccess,
});
