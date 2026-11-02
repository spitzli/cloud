import type { ComponentProps } from "react";
import { HostingActionBoundary } from "./action-boundary";
import { hostingUIScope } from "@webdock/module-hosting/ui/scope";
import { SetupHosting as SetupHostingView } from "@webdock/module-hosting/ui/setup-hosting";
export async function SetupHosting(
  props: ComponentProps<typeof SetupHostingView>,
) {
  return (
    <HostingActionBoundary key={hostingUIScope(props)}>
      <SetupHostingView {...props} />
    </HostingActionBoundary>
  );
}
