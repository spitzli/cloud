import { createMailResourcePage } from "@webdock/module-mail/ui/resource-page";
import { loadMailPage } from "./product-page";
import { mailRead } from "@/lib/mail-product";
import { getStudioSession } from "@/lib/studio-client";
import { MailProductForm } from "./product-form";
export const MailResourcePage = createMailResourcePage({
  loadMailPage,
  mailRead,
  getStudioSession,
  MailProductForm,
});
