"use client";
import type { ComponentProps } from "react";
import { DatabaseActionBoundary } from "../databases/action-boundary";
import { resourceScope as hostingUIScope } from "@spitzli/console-kit/resource-scope";
import {
  DatabaseOrderForm as DatabaseOrderFormView,
  DatabasePlanForm as DatabasePlanFormView,
} from "@spitzli/module-databases/ui/databases";
export type { DatabaseCatalogEntry } from "@spitzli/module-databases/ui/databases";
export function DatabaseOrderForm(
  props: ComponentProps<typeof DatabaseOrderFormView>,
) {
  return (
    <DatabaseActionBoundary key={hostingUIScope(props)}>
      <DatabaseOrderFormView {...props} />
    </DatabaseActionBoundary>
  );
}
export function DatabasePlanForm(
  props: ComponentProps<typeof DatabasePlanFormView>,
) {
  return (
    <DatabaseActionBoundary key={hostingUIScope(props)}>
      <DatabasePlanFormView {...props} />
    </DatabaseActionBoundary>
  );
}
