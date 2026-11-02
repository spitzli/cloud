import { hostingUIScope } from "@webdock/module-hosting/ui/scope";
import type { ComponentProps } from "react";
import { mailCall } from "@/lib/mail-product";
import { MailProductForm } from "@/components/mail/product-form";
import { createSetupMailPanelView } from "@webdock/module-hosting/ui/setup-mail-panel";
const { SetupMailPanel: SetupMailPanelView } = createSetupMailPanelView({
  mailCall,
  MailProductForm,
});
export async function SetupMailPanel(
  props: ComponentProps<typeof SetupMailPanelView>,
) {
  return <SetupMailPanelView key={hostingUIScope(props)} {...props} />;
}
