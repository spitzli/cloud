"use client";
import type { ComponentProps } from "react";
import { DatabaseActionBoundary } from "../databases/action-boundary";
import { resourceScope as hostingUIScope } from "@webdock/console-kit/resource-scope";
import { DatabaseCredentialPanel as DatabaseCredentialPanelView } from "@webdock/module-databases/ui/database-details";
export function DatabaseCredentialPanel(
  props: ComponentProps<typeof DatabaseCredentialPanelView>,
) {
  return (
    <DatabaseActionBoundary key={hostingUIScope(props)}>
      <DatabaseCredentialPanelView {...props} />
    </DatabaseActionBoundary>
  );
}
