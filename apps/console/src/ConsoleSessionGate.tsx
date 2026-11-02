// The production privacy gate; children are a slot so its real lifecycle can be qualified without router initialization.
import {LiveUpdates} from "./LiveUpdates";
import type { ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { ApiError, sessionSchema } from "./api";
import { SessionContext, useApi } from "./context";
import { ResetPassword, SignIn } from "./Auth";
import { PageFailure, PageProgress } from "./AuthFrame";
import { useSessionCacheIsolation } from "./session-cache";
import { useBackupIntentSession } from "./database-backup-intents";
export function ConsoleSessionGate({ children }: { children: ReactNode }) {
  const api = useApi();
  const session = useQuery({
    queryKey: ["session"],
    queryFn: ({ signal }) => api.get("/v1/auth/session", sessionSchema, signal),
    staleTime: 0,
    refetchInterval: 60000,
  });
  useBackupIntentSession(session.isSuccess ? session.data.data : undefined);
  const cacheReady = useSessionCacheIsolation(
    session.isError ? null : session.data?.data,
    session.dataUpdatedAt,
  );
  if (!cacheReady) return <PageProgress product="Console">Opening your workspace…</PageProgress>;
  if (window.location.pathname === "/reset-password") return <ResetPassword />;
  if (
    window.location.pathname === "/login" &&
    new URLSearchParams(window.location.search).has("error")
  )
    return <SignIn />;
  if (session.isPending || (!session.isFetchedAfterMount && !session.isError))
    return <PageProgress product="Console">Opening your workspace…</PageProgress>;
  if (session.isError)
    return session.error instanceof ApiError && session.error.status === 401 ? (
      <SignIn />
    ) : (
      <PageFailure
        product="Console"
        title="Workspace unavailable"
        error={session.error}
        retry={() => {
          void session.refetch();
        }}
      />
    );
  if (!session.data.data) return <SignIn />;
  return (
    <SessionContext.Provider value={session.data.data}>
      <LiveUpdates />
      {children}
    </SessionContext.Provider>
  );
}
