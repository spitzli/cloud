"use client";
import type { ComponentProps } from "react";
import { HostingActionBoundary } from "./action-boundary";
import { hostingUIScope } from "@webdock/module-hosting/ui/scope";
import {
  HostingForm as HostingFormView,
  EnrollmentForm as EnrollmentFormView,
} from "@webdock/module-hosting/ui/form";
export {
  HostingSaveContext,
  HostingPendingContext,
} from "@webdock/module-hosting/ui/form-state";
export function HostingForm(props: ComponentProps<typeof HostingFormView>) {
  return (
    <HostingActionBoundary key={hostingUIScope(props)}>
      <HostingFormView {...props} />
    </HostingActionBoundary>
  );
}
export function EnrollmentForm(
  props: ComponentProps<typeof EnrollmentFormView>,
) {
  return (
    <HostingActionBoundary key={hostingUIScope(props)}>
      <EnrollmentFormView {...props} />
    </HostingActionBoundary>
  );
}
