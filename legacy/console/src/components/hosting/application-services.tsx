import type { ComponentProps } from "react";
import { HostingActionBoundary } from "./action-boundary";
import { hostingUIScope } from "@webdock/module-hosting/ui/scope";
import { hostingPageCall } from "@/lib/hosting-client";
import { createApplicationServicesView } from "@webdock/module-hosting/ui/application-services";
const { ApplicationServices: ApplicationServicesView } =
  createApplicationServicesView({ hostingPageCall });
export async function ApplicationServices(
  props: ComponentProps<typeof ApplicationServicesView>,
) {
  return (
    <HostingActionBoundary key={hostingUIScope(props)}>
      <ApplicationServicesView {...props} />
    </HostingActionBoundary>
  );
}
