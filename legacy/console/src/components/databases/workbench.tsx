"use client";
import type { ComponentProps } from "react";
import { DatabaseWorkbench as View } from "@spitzli/module-databases/ui/workbench";
import { DatabaseActionBoundary } from "./action-boundary";
import { resourceScope } from "@spitzli/console-kit/resource-scope";
export function DatabaseWorkbench(props: ComponentProps<typeof View>) {
  return (
    <DatabaseActionBoundary key={resourceScope(props)}>
      <View {...props} />
    </DatabaseActionBoundary>
  );
}
