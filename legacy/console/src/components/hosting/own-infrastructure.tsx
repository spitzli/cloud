import type { ComponentProps } from "react";
import { HostingActionBoundary } from "./action-boundary";
import { hostingUIScope } from "@spitzli/module-hosting/ui/scope";
import { hostingPageCall } from "@/lib/hosting-client";
import { createOwnInfrastructureView } from "@spitzli/module-hosting/ui/own-infrastructure";
const { OwnInfrastructure: OwnInfrastructureView } =
  createOwnInfrastructureView({ hostingPageCall });
export async function OwnInfrastructure(
  props: ComponentProps<typeof OwnInfrastructureView>,
) {
  return (
    <HostingActionBoundary key={hostingUIScope(props)}>
      <OwnInfrastructureView {...props} />
    </HostingActionBoundary>
  );
}
