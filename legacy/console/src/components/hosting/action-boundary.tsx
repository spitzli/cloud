"use client";
import type { ReactNode } from "react";
import { HostingActionsProvider } from "@webdock/module-hosting/ui/actions";
import { saveHosting, displayEnrollment } from "@/lib/hosting-actions";
import { saveSetup, confirmSetup } from "@/lib/project-setup-actions";
const actions = { saveHosting, displayEnrollment, saveSetup, confirmSetup };
export function HostingActionBoundary({ children }: { children: ReactNode }) {
  return (
    <HostingActionsProvider actions={actions}>
      {children}
    </HostingActionsProvider>
  );
}
