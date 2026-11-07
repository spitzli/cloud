import { z } from "zod";
import { useQuery } from "@tanstack/react-query";
import { useApi, useSession } from "../context";

// Discovery is scoped by the Core to the project and the viewer's self-service access;
// admission is rechecked on every call.
const labelled = z.object({ id: z.string(), label: z.string() });
const schemaNode: z.ZodType<JsonSchema> = z.lazy(() =>
  z.object({
    type: z.string(),
    properties: z.record(z.string(), schemaNode).optional(),
    required: z.array(z.string()).optional(),
    enum: z.array(z.union([z.string(), z.number(), z.boolean(), z.null()])).optional(),
    description: z.string().optional(),
    minLength: z.number().optional(),
    maxLength: z.number().optional(),
    minimum: z.number().optional(),
    maximum: z.number().optional(),
  }),
);
export interface JsonSchema {
  type: string;
  properties?: Record<string, JsonSchema> | undefined;
  required?: string[] | undefined;
  enum?: (string | number | boolean | null)[] | undefined;
  description?: string | undefined;
  minLength?: number | undefined;
  maxLength?: number | undefined;
  minimum?: number | undefined;
  maximum?: number | undefined;
}
const actionSchema = z.object({
  name: z.string(),
  intent: z.enum(["read", "write"]),
  audience: z.enum(["operator", "customer"]),
  offering: z.string().optional(),
  feature: z.string().optional(),
  resource: z.enum(["create", "use", "delete"]).optional(),
  input: schemaNode.optional(),
});
const pageSchema = z.object({ id: z.string(), label: z.string(), action: z.string(), resource: z.boolean().optional() });
export const pluginViewSchema = z.object({
  state: z.string(),
  runtime: z.string(),
  manifest: z.object({
    name: z.string(),
    description: z.string().optional(),
    offerings: z.array(labelled.extend({ description: z.string().optional(), highlights: z.array(z.string()).max(6).optional() })).max(16),
    selfService: z.object({ features: z.array(labelled.extend({ offering: z.string().optional(), description: z.string().optional() })).max(32) }),
    actions: z.array(actionSchema).max(64),
    areas: z.object({
      operator: z.object({ pages: z.array(pageSchema).max(16) }),
      selfService: z.object({ pages: z.array(pageSchema).max(16) }).optional(),
    }),
  }),
  features: z.array(z.string()).max(32),
  blockedCreateActions: z.array(z.string().max(64)).max(64).optional(),
  resources: z.array(z.object({ id: z.string(), offering: z.string(), name: z.string(), createdVia: z.enum(["operator", "self-service"]) })).max(1000),
});
export type PluginView = z.infer<typeof pluginViewSchema>;
export type PluginAction = z.infer<typeof actionSchema>;

export function usePluginViews(projectID: string, enabled = true) {
  const api = useApi();
  const session = useSession();
  return useQuery({
    queryKey: ["project-plugins", session.session.id, projectID],
    enabled,
    queryFn: ({ signal }) =>
      api.get(`/api/console/plugins?projectID=${encodeURIComponent(projectID)}`, z.array(pluginViewSchema).max(64), signal),
  });
}
/** Create actions this viewer may run here, each with its feature and offering labels. */
export function creatable(view: PluginView) {
  return view.manifest.actions.flatMap((action) => {
    if (action.resource !== "create" || !action.feature || !view.features.includes(action.feature) || view.blockedCreateActions?.includes(action.name)) return [];
    const feature = view.manifest.selfService.features.find((f) => f.id === action.feature);
    const offering = view.manifest.offerings.find((o) => o.id === action.offering);
    return feature && offering ? [{ action, feature, offering }] : [];
  });
}
export function invokePlugin(
  api: ReturnType<typeof useApi>,
  plugin: string,
  projectID: string,
  action: string,
  input: Record<string, unknown>,
  signal?: AbortSignal,
  idempotencyKey?: string,
) {
  return api.request("/api/console/plugins/invoke", z.unknown(), {
    method: "POST",
    signal: signal ?? null,
    body: JSON.stringify({ pluginID: plugin, projectID, action, input, idempotencyKey: idempotencyKey ?? crypto.randomUUID() }),
  });
}
export const projectSchema = z.object({
  url: z.string().nullish(),
  repositoryURL: z.string().nullish(),
  id: z.string(),
  name: z.string(),
  customer: z.object({ id: z.string(), name: z.string().nullable() }),
  role: z.string().nullable(),
});
export function useProject(projectID: string) {
  const api = useApi();
  return useQuery({
    queryKey: ["project", projectID],
    queryFn: ({ signal }) => api.get(`/api/console/project/${encodeURIComponent(projectID)}`, projectSchema, signal),
  });
}
export const revisionSchema = z.object({ revision: z.string() });
