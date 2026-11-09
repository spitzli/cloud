import { queryOptions, type QueryClient } from "@tanstack/react-query";
import { z } from "zod";
import type { ApiClient, Session } from "./api";
import { sessionCacheScope } from "./session-cache";
export const projectSchema = z.object({
  url: z.string().nullish(), repositoryURL: z.string().nullish(),
  id: z.string(), name: z.string(),
  customer: z.object({ id: z.string(), name: z.string().nullable() }),
  role: z.string().nullable(),
});
export function projectQuery(api: ApiClient, session: Session, projectID: string) {
  return queryOptions({
    queryKey: ["project", projectID, sessionCacheScope(session), api.config.workspaceApiOrigin ?? api.config.apiOrigin] as const,
    queryFn: ({ signal }) => api.get(`/api/console/project/${encodeURIComponent(projectID)}`, projectSchema, signal),
    staleTime: 15_000,
  });
}
export async function prefetchProjectSummary(client: QueryClient, api: ApiClient, session: Session, projectID: string): Promise<void> {
  const state = client.getQueryState<{ data: Session | null }>(["session"]);
  const current = state?.status === "success" && !state.isInvalidated ? state.data?.data : null;
  const expires = Date.parse(session.session.expiresAt);
  if (!Number.isFinite(expires) || expires <= Date.now() || !current || sessionCacheScope(current) !== sessionCacheScope(session) ||
    !session.user.emailVerified || session.user.mustChangePassword || session.user.preview ||
    !(session.user.role === "user" || session.user.role === "operator" && session.user.twoFactorEnabled) ||
    !/^[1-9][0-9]{0,18}$/.test(projectID)) return;
  await client.query({ ...projectQuery(api, session, projectID), retry: false }).catch(() => undefined);
}
