import type { ModuleView } from "@webdock/module-sdk";
/** Optional dashboard data: disabled products are not contacted; active-provider failures still surface. */
export function optionalModuleData<T>(
  view: ModuleView | undefined,
  id: string,
  load: () => Promise<T>,
): Promise<T | null> {
  return ["enabled", "draining"].includes(view?.modules[id] ?? "")
    ? load()
    : Promise.resolve(null);
}

export function hasProductModules(view: ModuleView | undefined): boolean {
  return ["mail", "hosting", "databases", "cms", "storage"].some(id => ["enabled", "draining"].includes(view?.modules[id] ?? ""));
}
