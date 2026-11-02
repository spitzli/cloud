import { useQueries, useQuery } from "@tanstack/react-query";
import type { ComponentType } from "react";
import { z } from "zod";
import { DatabaseIcon } from "@phosphor-icons/react/dist/csr/Database";
import { EnvelopeIcon } from "@phosphor-icons/react/dist/csr/Envelope";
import { AppWindowIcon } from "@phosphor-icons/react/dist/csr/AppWindow";
import { NoteIcon } from "@phosphor-icons/react/dist/csr/Note";
import { CubeIcon } from "@phosphor-icons/react/dist/csr/Cube";
import { useApi, useSession } from "../context";
import { ApiError } from "../api";
import { pluginViewSchema, type PluginView } from "./views";
import { pluginNavMeta } from "./registry";

// Sidebar label and icon of first-party plugins (`@plugins/<id>/ui/nav.ts`); others fall back to their name.
const icons: Record<string, ComponentType<{ size?: number }>> = { database: DatabaseIcon, envelope: EnvelopeIcon, app: AppWindowIcon, note: NoteIcon };
export function pluginNav(plugin: string) {
  const nav = pluginNavMeta(plugin);
  return { label: nav?.label ?? plugin.charAt(0).toUpperCase() + plugin.slice(1), Icon: icons[nav?.icon ?? ""] ?? CubeIcon };
}

export const workspaceProjectsSchema = z.array(z.object({ id: z.string(), name: z.string(), status: z.string() }));
export type WorkspaceProject = z.infer<typeof workspaceProjectsSchema>[number];
export type WorkspaceResource = PluginView["resources"][number] & { project: WorkspaceProject };
export interface WorkspacePlugin {
  plugin: string;
  view: PluginView;
  /** Features this viewer may use in this workspace (the access mode is per customer, not per project). */
  features: string[];
  resources: WorkspaceResource[];
}

export function useWorkspaceProjects(customer: string) {
  const api = useApi();
  return useQuery({
    queryKey: ["workspace-projects", customer],
    queryFn: ({ signal }) => api.get(`/api/console/tenant/${encodeURIComponent(customer)}/projects`, workspaceProjectsSchema, signal),
  });
}
/** Every plugin with a self-service area, merged across the workspace's projects.
 *  ponytail: one discovery call per project; a workspace-level endpoint when workspaces hold many projects. */
export function useWorkspacePlugins(customer: string) {
  const api = useApi();
  const session = useSession();
  const projects = useWorkspaceProjects(customer);
  const views = useQueries({
    queries: (projects.data ?? []).map((project) => ({
      queryKey: ["project-plugins", session.session.id, project.id],
      queryFn: ({ signal }: { signal: AbortSignal }) => api.get(`/api/console/plugins?projectID=${encodeURIComponent(project.id)}`, z.array(pluginViewSchema).max(64), signal),
    })),
  });
  const pending = projects.isPending || views.some((v) => v.isPending);
  // Installations without a plugin host answer "unavailable"; the workspace still opens.
  const error = projects.error ?? views.find((v) => v.error && !(v.error instanceof ApiError && [404, 503].includes(v.error.status)))?.error ?? null;
  const merged = new Map<string, WorkspacePlugin>();
  (projects.data ?? []).forEach((project, index) => {
    for (const view of views[index]?.data ?? []) {
      if (!view.manifest.areas.selfService) continue;
      const entry = merged.get(view.manifest.name) ?? { plugin: view.manifest.name, view, features: [], resources: [] };
      entry.features = [...new Set([...entry.features, ...view.features])];
      entry.resources.push(...view.resources.map((resource) => ({ ...resource, project })));
      merged.set(view.manifest.name, entry);
    }
  });
  return { projects, plugins: [...merged.values()].sort((a, b) => pluginNav(a.plugin).label.localeCompare(pluginNav(b.plugin).label)), pending, error };
}
