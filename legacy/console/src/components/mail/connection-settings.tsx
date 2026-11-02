import { createMailConnectionSettings } from "@webdock/module-mail/ui/connection-settings";
import { loadMailPage } from "./product-page";
import { mailRead } from "@/lib/mail-product";
import { getStudioSession } from "@/lib/studio-client";
import { MailProductForm } from "./product-form";
export const MailConnectionSettings = createMailConnectionSettings({
  loadMailPage,
  mailRead,
  getStudioSession,
  MailProductForm,
});
