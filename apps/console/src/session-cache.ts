import { useEffect, useState } from "react";
import {
  useQueryClient,
  type QueryClient,
  type QueryKey,
} from "@tanstack/react-query";
import type { Session } from "./api";

function isPrivate(query: { queryKey: QueryKey }): boolean {
  const key = query.queryKey[0];
  return (
    key !== "runtime-config" && key !== "session" && key !== "account-session"
  );
}

export async function retirePrivateCache(client: QueryClient): Promise<void> {
  const mutations = client.getMutationCache();
  // Clearing a MutationCache does not cancel execution or its option callbacks.
  // Retire callbacks that have not started. Already-dispatched server writes
  // and callback bodies already executing cannot be undone here.
  for (const mutation of mutations.getAll()) {
    mutation.setOptions({
      mutationFn: () => Promise.reject(new Error("Session authority changed.")),
      retry: false,
      gcTime: 0,
    });
  }
  mutations.clear();
  await client.cancelQueries({ predicate: isPrivate });
  client.removeQueries({ predicate: isPrivate });
}

export function sessionCacheScope(
  session: Session | null | undefined,
  verifiedAt = 0,
): string {
  if (!session) return "anonymous";
  return JSON.stringify([
    session.user.id,
    session.session.userId,
    session.session.id,
    session.session.expiresAt,
    session.user.role ?? null,
    session.user.emailVerified,
    session.user.mustChangePassword ?? false,
    session.user.twoFactorEnabled ?? false,
    session.user.preview ?? false,
    session.user.acting ?? false,
    session.user.customerScope ?? null,
    // The wire contract exposes no preview target/version yet. Each confirmed
    // preview response is a new scope, including true-to-true target changes.
    session.user.preview ? verifiedAt : 0,
  ]);
}

export function useSessionCacheIsolation(
  session: Session | null | undefined,
  verifiedAt = 0,
): boolean {
  const client = useQueryClient();
  const scope = sessionCacheScope(session, verifiedAt);
  const [boundary, setBoundary] = useState({ scope, ready: false });
  // Render the barrier immediately; an effect-only purge permits one stale paint.
  if (boundary.scope !== scope) setBoundary({ scope, ready: false });
  useEffect(() => {
    let current = true;
    void retirePrivateCache(client).then(() => {
      if (current) setBoundary({ scope, ready: true });
    });
    return () => {
      current = false;
    };
  }, [client, scope]);
  return boundary.scope === scope && boundary.ready;
}
