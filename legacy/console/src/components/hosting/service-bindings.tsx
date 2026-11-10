"use client";
import type { ComponentProps } from "react";
import { HostingActionBoundary } from "./action-boundary";
import { hostingUIScope } from "@spitzli/module-hosting/ui/scope";
import { ConnectService as ConnectServiceView } from "@spitzli/module-hosting/ui/service-bindings";
export function ConnectService(
  props: ComponentProps<typeof ConnectServiceView>,
) {
  return (
    <HostingActionBoundary key={hostingUIScope(props)}>
      <ConnectServiceView {...props} />
    </HostingActionBoundary>
  );
}
