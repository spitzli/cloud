import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { databaseBackupPolicyViewSchema } from "@webdock/hosting-contracts";
import { z } from "zod";
import { useApi } from "./context";
import type { BackupRequestMemory } from "./database-backup-intents";

export function useDatabaseBackupHistory(memory: BackupRequestMemory, scope: string, databaseID: string, dedicated: boolean, readable: boolean, revalidate: () => Promise<boolean>, readDenied: () => void) {
  const api = useApi(), client = useQueryClient();
  const [latched, setLatched] = useState(false);
  const [readFence, setReadFence] = useState(0);
  const generation = useRef(0), fence = useRef(0), mounted = useRef(false);
  useLayoutEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const alive = useCallback(() => mounted.current, []);
  const canRead = readable && !latched && !memory.snapshot.readDenied;
  const history = useQuery({
    queryKey: ["database-backups", scope],
    queryFn: async ({ signal }) => {
      const read = ++generation.current;
      try {
        const value = await api.request("/api/console/databases", z.object({ data: databaseBackupPolicyViewSchema }), { method: "POST", body: JSON.stringify({ action: "databases.backupPolicies.get", databaseID }), signal });
        return { view: value.data, read, receivedAt: Date.now() };
      } catch (error) {
        if (mounted.current && !signal.aborted) { fence.current = generation.current; setReadFence(generation.current); setLatched(true); memory.cell.denyRead(); readDenied(); }
        throw error;
      }
    },
    enabled: dedicated && canRead,
    retry: false, refetchOnWindowFocus: false, refetchOnReconnect: false,
    refetchInterval: canRead ? 10000 : false,
  });
  useLayoutEffect(() => {
    if (canRead) return;
    const through = generation.current; fence.current = through;
    void client.cancelQueries({ queryKey: ["database-backups", scope] }).then(() => { if (alive()) setReadFence(prior => Math.max(prior, through)); });
    client.removeQueries({ queryKey: ["database-backups", scope] });
  }, [canRead, client, scope, alive]);
  useLayoutEffect(() => { if (!readable && !memory.cell.snapshot().readDenied) memory.cell.denyRead(); }, [readable, memory.cell]);
  useLayoutEffect(() => () => { void client.cancelQueries({ queryKey: ["database-backups", scope] }); client.removeQueries({ queryKey: ["database-backups", scope] }); }, [client, scope]);
  const refresh = useCallback(async (current: () => boolean) => {
    if (!alive() || !current() || !await revalidate() || !alive() || !current()) return false;
    setLatched(false);
    const result = await history.refetch();
    return alive() && current() && result.isSuccess && result.data.read > fence.current;
  }, [revalidate, history, alive]);
  return {
    data: canRead && history.isSuccess && history.data.read > readFence ? history.data.view : undefined,
    receivedAt: history.isSuccess ? history.data.receivedAt : undefined,
    unavailable: !canRead || history.isError,
    refresh,
    observe: () => { if (canRead) void history.refetch(); },
  };
}
