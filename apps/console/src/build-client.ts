import { useCallback } from "react";
import type { z } from "zod";
import type { ApiClient } from "./api";
import { useApi } from "./context";
import { buildCommandSchema, buildID, logCursor, logPageSchema } from "./build-contracts";

export function useBuildCommand() {
  const api = useApi();
  return useCallback(<T>(input: unknown, schema: z.ZodType<T>, signal?: AbortSignal) => api.request("/api/console/hosting", schema, { method: "POST", body: JSON.stringify(buildCommandSchema.parse(input)), signal: signal ?? null }), [api]);
}
export function buildLogPath(projectID: string, id: string, after: string): string {
  return `/api/console/hosting/projects/${buildID.parse(projectID)}/builds/${buildID.parse(id)}/logs?after=${logCursor.parse(after)}`;
}
export function buildLogEventsPath(projectID:string,id:string):string {return `/api/console/hosting/projects/${buildID.parse(projectID)}/builds/${buildID.parse(id)}/logs/events`;}
export function readBuildLogs(api: ApiClient, projectID: string, id: string, after: string, signal?: AbortSignal) {
  return api.get(buildLogPath(projectID, id, after), logPageSchema, signal);
}
