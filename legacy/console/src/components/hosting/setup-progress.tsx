import type { ComponentProps } from "react";
import { HostingActionBoundary } from "./action-boundary";
import { hostingUIScope } from "@webdock/module-hosting/ui/scope";
import { SetupProgress as SetupProgressView } from "@webdock/module-hosting/ui/setup-progress";
export async function SetupProgress(
  props: ComponentProps<typeof SetupProgressView>,
) {
  return (
    <HostingActionBoundary key={hostingUIScope(props)}>
      <SetupProgressView {...props} />
    </HostingActionBoundary>
  );
}
