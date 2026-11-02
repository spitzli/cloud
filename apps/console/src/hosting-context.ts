import { useQuery } from "@tanstack/react-query";
import type { z } from "zod";
import { useApi } from "./context";
import { useHostingCommand } from "./hosting-client";
import {
  appSchema,
  operationSchema,
  projectSchema,
  tenantsSchema,
} from "./hosting-contracts";
export function hostingLocation(path: string): {
  kind: "none" | "directory" | "project" | "app" | "operation" | "invalid";
  id: string | undefined;
} {
  if (path === "/hosting") return { kind: "directory", id: undefined };
  if (!path.startsWith("/hosting/")) return { kind: "none", id: undefined };
  const project = /^\/hosting\/projects\/([1-9][0-9]{0,18})(?:\/builds)?$/.exec(path)?.[1];
  if (project) return { kind: "project", id: project };
  const app =
    /^\/hosting\/apps\/([1-9][0-9]{0,18})(?:\/(?:settings|logs))?$/.exec(
      path,
    )?.[1];
  if (app) return { kind: "app", id: app };
  const operation = /^\/hosting\/operations\/([1-9][0-9]{0,18})$/.exec(
    path,
  )?.[1];
  if (operation) return { kind: "operation", id: operation };
  return { kind: "invalid", id: undefined };
}
export function useHostingContext(
  path: string,
  customerSelector: string | undefined,
) {
  const api = useApi();
  const call = useHostingCommand();
  const location = hostingLocation(path);
  const active = location.kind !== "none";
  const enabled = active && api.config.enabledFeatures.includes("hosting");
  const appID = location.kind === "app" ? location.id : undefined;
  const operationID = location.kind === "operation" ? location.id : undefined;
  const application = useQuery({
    queryKey: ["hosting", "app", appID],
    queryFn: ({ signal }) =>
      call({ action: "apps.get", appID }, appSchema, signal),
    enabled: enabled && Boolean(appID),
  });
  const operation = useQuery({
    queryKey: ["hosting-operation", operationID],
    queryFn: ({ signal }) =>
      call({ action: "operations.get", operationID }, operationSchema, signal),
    enabled: enabled && Boolean(operationID),
  });
  const projectID = projectSelector(location, application.data, operation.data);
  const project = useQuery({
    queryKey: ["hosting", "project", projectID],
    queryFn: ({ signal }) =>
      call({ action: "projects.get", projectID }, projectSchema, signal),
    enabled: enabled && Boolean(projectID),
  });
  const tenantTarget = tenantSelector(
    location,
    customerSelector,
    operation.data,
  );
  const needsTenants = Boolean(tenantTarget) && !projectID;
  const tenants = useQuery({
    queryKey: ["tenants"],
    queryFn: ({ signal }) =>
      api.get("/api/console/tenants", tenantsSchema, signal),
    enabled: enabled && needsTenants,
  });
  const customer = customerContext(project.data, tenants.data, tenantTarget);
  const statuses = [
    readStatus(Boolean(appID), application),
    readStatus(Boolean(operationID), operation),
    readStatus(Boolean(projectID), project),
    readStatus(needsTenants, tenants),
  ];
  const pending = enabled && statuses.some((status) => status.pending);
  const missingTenant = needsTenants && !pending && !customer;
  const unavailable = contextUnavailable(
    location,
    enabled,
    statuses.some((status) => status.failed),
    missingTenant,
  );
  const visible = visibleContext(unavailable, {
    project: project.data,
    application: application.data,
    customer,
  });
  const label = workspaceLabel(unavailable, pending, visible.customer?.name);
  return {
    active,
    ...visible,
    pending,
    unavailable,
    label,
    breadcrumbs: [
      label,
      visible.project?.name,
      visible.application?.name,
      hostingArea(path, location.kind),
    ].filter((value): value is string => value !== undefined),
  };
}
function workspaceLabel(
  unavailable: boolean,
  pending: boolean,
  name: string | undefined,
) {
  if (unavailable) return "Workspace unavailable";
  if (pending) return "Loading workspace…";
  return name ?? "All workspaces";
}
function hostingArea(
  path: string,
  kind: ReturnType<typeof hostingLocation>["kind"],
) {
  if (path.endsWith("/builds")) return "Builds";
  if (path.endsWith("/settings")) return "Settings";
  if (path.endsWith("/logs")) return "Logs";
  if (kind === "operation") return "Operation";
  if (kind === "app") return "Overview";
  return "Applications";
}

type Location = ReturnType<typeof hostingLocation>;
type App = z.infer<typeof appSchema>;
type Operation = z.infer<typeof operationSchema>;
type Project = z.infer<typeof projectSchema>;
function projectSelector(
  location: Location,
  app: App | undefined,
  operation: Operation | undefined,
) {
  if (location.kind === "project") return location.id;
  return app?.projectID ?? operation?.projectID ?? undefined;
}
function tenantSelector(
  location: Location,
  customer: string | undefined,
  operation: Operation | undefined,
) {
  if (location.kind === "directory") return customer;
  return operation?.customerID ?? undefined;
}
function customerContext(
  project: Project | undefined,
  tenants: z.infer<typeof tenantsSchema> | undefined,
  target: string | undefined,
) {
  if (project) return { id: project.customerID, name: project.customerName };
  return tenants?.find((tenant) => tenant.id === target);
}
function readStatus(
  required: boolean,
  query: { isPending: boolean; error: Error | null },
) {
  if (!required) return { pending: false, failed: false };
  return { pending: query.isPending, failed: query.error !== null };
}
function contextUnavailable(
  location: Location,
  enabled: boolean,
  failed: boolean,
  missingTenant: boolean,
) {
  if (location.kind === "none") return false;
  return !enabled || location.kind === "invalid" || failed || missingTenant;
}

function visibleContext(
  unavailable: boolean,
  context: {
    project: Project | undefined;
    application: App | undefined;
    customer: { id: string; name: string } | undefined;
  },
) {
  // TanStack retains stale data after an error; a failed required lookup hides the whole scope.
  if (unavailable)
    return { project: undefined, application: undefined, customer: undefined };
  return context;
}
