import type { ComponentProps } from "react";
import { HostingActionBoundary } from "./action-boundary";
import { hostingUIScope } from "@spitzli/module-hosting/ui/scope";
import { OwnInfrastructure } from "./own-infrastructure";
import { hostingPageCall } from "@/lib/hosting-client";
import { getAvailableProjects } from "./view-services";
import { createCustomerView } from "@spitzli/module-hosting/ui/customer";
const { CustomerHosting: CustomerHostingView } = createCustomerView({
  getAvailableProjects,
  OwnInfrastructure,
  hostingPageCall,
});
export async function CustomerHosting(
  props: ComponentProps<typeof CustomerHostingView>,
) {
  return (
    <HostingActionBoundary key={hostingUIScope(props)}>
      <CustomerHostingView {...props} />
    </HostingActionBoundary>
  );
}
