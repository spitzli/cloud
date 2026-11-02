import { createMailContent } from "@webdock/module-mail/server/product-content";
import { mailDatabase as database } from "./store";
export const {
  sealMailAttachments,
  openMailAttachments,
  eraseAttachmentContent,
  discardExpiredUncertainContent,
} = createMailContent({
  database,
  contentSecret: () => process.env.BETTER_AUTH_SECRET,
});
