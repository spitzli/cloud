"use client";
import type { ComponentProps } from "react";
import { HostingActionBoundary } from "./action-boundary";
import { hostingUIScope } from "@spitzli/module-hosting/ui/scope";
import { SetupEnvironmentRepair as SetupEnvironmentRepairView } from "@spitzli/module-hosting/ui/setup-environment-repair";
export function SetupEnvironmentRepair(
  props: ComponentProps<typeof SetupEnvironmentRepairView>,
) {
  return (
    <HostingActionBoundary key={hostingUIScope(props)}>
      <SetupEnvironmentRepairView {...props} />
    </HostingActionBoundary>
  );
}
