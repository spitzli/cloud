import type { ComponentProps } from "react";
import { HostingActionBoundary } from "./action-boundary";
import { hostingUIScope } from "@spitzli/module-hosting/ui/scope";
import { SetupProgress as SetupProgressView } from "@spitzli/module-hosting/ui/setup-progress";
export async function SetupProgress(
  props: ComponentProps<typeof SetupProgressView>,
) {
  return (
    <HostingActionBoundary key={hostingUIScope(props)}>
      <SetupProgressView {...props} />
    </HostingActionBoundary>
  );
}
