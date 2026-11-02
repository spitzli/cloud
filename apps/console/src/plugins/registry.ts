// First-party plugin code the Console bundles from `@plugins/<id>/ui` (ponytail: runtime loading comes
// with third-party React plugins). Both globs resolve on first
// use: Vite replaces them at build time, while plain module runners (unit tests) have none.
import type { PluginUI } from "./sdk";

interface Nav { default: { label: string; icon: string } }
function byPlugin<T>(load: () => Record<string, T>): Record<string, T | undefined> {
  try {
    return Object.fromEntries(Object.entries(load()).flatMap(([path, module]) => {
      const id = /([a-z][a-z0-9-]*)\/ui\/[^/]+$/.exec(path)?.[1];
      return id ? [[id, module]] : [];
    }));
  } catch {
    return {};
  }
}
let views: Record<string, (() => Promise<{ default: PluginUI }>) | undefined> | undefined;
let navs: Record<string, Nav | undefined> | undefined;
export function pluginUILoader(plugin: string) {
  views ??= byPlugin(() => import.meta.glob<{ default: PluginUI }>("@plugins/*/ui/index.tsx"));
  return views[plugin];
}
export function pluginNavMeta(plugin: string) {
  navs ??= byPlugin(() => import.meta.glob<Nav>("@plugins/*/ui/nav.ts", { eager: true }));
  return navs[plugin]?.default;
}
