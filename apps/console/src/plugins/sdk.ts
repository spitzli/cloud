// The surface first-party plugin UIs (`plugins/<id>/ui`) import as `@webdock/plugin-ui`.
// Every call goes through the plugin action API, so the Core gates apply exactly as for panels.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useContext, useEffect, useMemo, useRef, useState, type ComponentType } from "react";
import type { z } from "zod";
import { SessionContext, useApi } from "../context";
import { sessionCacheScope } from "../session-cache";
import { invokePlugin } from "./views";
export * as ui from "../ui";
export { Empty, Failure, Loading } from "../common";
export { usePluginStorage, usePluginScope, storageKeySchema, storageHashSchema, STORAGE_CONTENT_TYPES, type TransferDescriptor, type BinaryTransferOptions } from "./storage";

export interface PluginContext { plugin: string; projectID: string }
export interface PluginResource { id: string; offering: string; name: string; createdVia: "operator" | "self-service" }
export interface ResourceProps extends PluginContext {
  customer: string;
  resource: PluginResource;
  /** Self-service features this viewer may use here (operators: all). */
  features: string[];
  /** Whether the viewer may run write actions at all (owner, admin or operator). */
  writable: boolean;
  /** Actions declared by the installed release; absent on older host adapters. */
  actions?: readonly string[];
  /** Call after a delete action succeeded; returns to the project. */
  onDeleted: () => void;
}
export interface OperatorResource extends Omit<PluginResource, "createdVia"> {
  createdVia: PluginResource["createdVia"];
  customer: { id: string; name: string };
  project: { id: string; name: string };
}
export interface OperatorProps { plugin: string; resources: OperatorResource[] }
export interface CreateProps extends PluginContext {
  customer: string;
  /** The create action this form runs. */
  action: string;
  title: string;
  /** This plugin's resources in the project, e.g. to pick a domain. */
  resources: PluginResource[];
  /** Call with the created resource id; the Console opens it. */
  onCreated: (resource: string) => void;
  onCancel: () => void;
}
/** What a plugin UI module default-exports. Missing pages fall back to declarative panels and
 *  generated create forms. `Create` is keyed by create action name. */
export interface PluginUI {
  Resource?: ComponentType<ResourceProps>;
  Operator?: ComponentType<OperatorProps>;
  Create?: Partial<Record<string, ComponentType<CreateProps>>>;
}

export function usePluginQuery<T>(
  context: PluginContext,
  action: string,
  input: Record<string, unknown>,
  schema: z.ZodType<T>,
  options: { refetchInterval?: (data: T | undefined) => number | false } = {},
) {
  const api = useApi();
  return useQuery({
    queryKey: ["plugin-ui", context.plugin, context.projectID, action, input],
    queryFn: async ({ signal }) => schema.parse(await invokePlugin(api, context.plugin, context.projectID, action, input, signal)),
    retry: false,
    refetchInterval: (query) => options.refetchInterval?.(query.state.data) ?? false,
  });
}
/** A write; on success every query of this plugin in this project refetches. A delete passes
 *  `leaves: true` so nothing refetches the resource it just removed. */
export function usePluginMutation(context: PluginContext, action: string, options: { leaves?: boolean } = {}) {
  const api = useApi();
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: Record<string, unknown>) => invokePlugin(api, context.plugin, context.projectID, action, input),
    onSuccess: () => options.leaves ? undefined : client.invalidateQueries({ queryKey: ["plugin-ui", context.plugin, context.projectID] }),
  });
}
/** One logical submission keeps its explicit key when a response is lost. */
export interface PluginSubmission { input: Record<string, unknown>; idempotencyKey: string; signal?: AbortSignal }
export function usePluginSubmission<T>(context: PluginContext, action: string, schema: z.ZodType<T>) {
  const api = useApi();
  const client = useQueryClient();
  return useMutation({
    mutationFn: async ({ input, idempotencyKey, signal }: PluginSubmission) => schema.parse(await invokePlugin(api, context.plugin, context.projectID, action, input, signal, idempotencyKey)),
    retry: false,
    gcTime: 0,
    onSettled: () => client.invalidateQueries({ queryKey: ["plugin-ui", context.plugin, context.projectID] }),
  });
}
/** Sensitive results belong only to the mounted caller, never a shared query/mutation cache. */
export function usePluginReveal(context: PluginContext & { resource?: Pick<PluginResource, "id"> }, action: string) {
  const api = useApi();
  const session = useContext(SessionContext);
  const key = JSON.stringify([context.plugin, context.projectID, action, context.resource?.id, sessionCacheScope(session)]);
  const scope = useMemo(() => ({ api, key }), [api, key]);
  interface State { scope: typeof scope; data: unknown; error: Error | null; status: "idle" | "pending" | "success" | "error" }
  const empty = (): State => ({ scope, data: undefined, error: null, status: "idle" });
  const [state, setState] = useState<State>(empty);
  const active = useRef<{ scope: typeof scope; controller: AbortController } | null>(null);
  // Clear before a changed principal/project/resource can paint the old value.
  if (state.scope !== scope) setState(empty());
  useEffect(() => () => {
    if (active.current?.scope === scope) {
      const previous = active.current;
      active.current = null;
      previous.controller.abort();
    }
  }, [scope]);
  const reset = () => {
    const previous = active.current;
    active.current = null;
    previous?.controller.abort();
    setState(empty());
  };
  const mutateAsync = async (input: Record<string, unknown>): Promise<unknown> => {
    active.current?.controller.abort();
    const request = { scope, controller: new AbortController() };
    active.current = request;
    setState({ scope, data: undefined, error: null, status: "pending" });
    try {
      const result = await invokePlugin(api, context.plugin, context.projectID, action, input, request.controller.signal);
      if (active.current !== request) throw new DOMException("Reveal cancelled", "AbortError");
      active.current = null;
      setState({ scope, data: result, error: null, status: "success" });
      return result;
    } catch (error) {
      const failure = error instanceof Error ? error : new Error("Could not load the private value.");
      if (active.current === request) {
        active.current = null;
        setState({ scope, data: undefined, error: failure, status: "error" });
      }
      throw failure;
    }
  };
  const visible = state.scope === scope ? state : empty();
  return {
    data: visible.data, error: visible.error, status: visible.status,
    isPending: visible.status === "pending", isSuccess: visible.status === "success",
    isError: visible.status === "error", isIdle: visible.status === "idle",
    reset, mutateAsync,
    mutate: (input: Record<string, unknown>) => { void mutateAsync(input).catch(() => undefined); },
  };
}
