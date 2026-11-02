import { createMailGatewayOverview } from "@webdock/module-mail/ui/gateway-overview";
import { loadMailPage } from "./product-page";
import { mailRead } from "@/lib/mail-product";
import { getStudioSession } from "@/lib/studio-client";
import { MailProductForm } from "./product-form";
export const MailGatewayOverview = createMailGatewayOverview({
  loadMailPage,
  mailRead,
  getStudioSession,
  MailProductForm,
});
