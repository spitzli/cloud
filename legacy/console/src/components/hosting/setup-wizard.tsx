"use client";
import type { ComponentProps } from "react";
import { HostingActionBoundary } from "./action-boundary";
import { hostingUIScope } from "@webdock/module-hosting/ui/scope";
import { SetupWizard as SetupWizardView } from "@webdock/module-hosting/ui/setup-wizard";
export function SetupWizard(props: ComponentProps<typeof SetupWizardView>) {
  return (
    <HostingActionBoundary key={hostingUIScope(props)}>
      <SetupWizardView {...props} />
    </HostingActionBoundary>
  );
}
