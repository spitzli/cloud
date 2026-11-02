import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { useApi, useSession } from "./context";
import * as ui from "./ui";

const scopeSchema = z.object({
  customerID: z.string(), customerName: z.string(), status: z.enum(["active", "expired"]),
  mode: z.enum(["read_only", "owner"]), readOnly: z.boolean(), expiresAt: z.string(),
});

/** Starting either mode retires operator caches before opening the scoped workspace. */
export function CustomerScopeStart({ customer }: { customer: string }) {
  const api = useApi();
  const client = useQueryClient();
  const start = useMutation({
    mutationFn: (mode: "read_only" | "owner") => api.write("/api/console/tenant-preview/start", "POST", { customerID: customer, mode }, scopeSchema),
    onSuccess: (scope) => { client.clear(); location.assign(`/workspaces/${scope.customerID}`); },
  });
  return <div className="mb-6">
    <div className="flex flex-wrap gap-2">
      <button className={ui.button()} disabled={start.isPending} onClick={() => { start.mutate("owner"); }}>Act as customer owner</button>
      <button className={ui.button("quiet")} disabled={start.isPending} onClick={() => { start.mutate("read_only"); }}>Preview read-only</button>
    </div>
    {start.error ? <p role="alert" className="mt-2 text-sm text-danger">{start.error.message}</p> : null}
  </div>;
}

/** Visible on every Console route while the real operator session is customer-scoped. */
export function CustomerScopeBanner() {
  const api = useApi();
  const client = useQueryClient();
  const session = useSession();
  const scope = useQuery({ queryKey: ["customer-scope"], queryFn: () => api.write("/api/console/tenant-preview/current", "POST", {}, scopeSchema), enabled: Boolean(session.user.preview), refetchInterval: 30_000 });
  const exit = useMutation({
    mutationFn: () => api.write("/api/console/tenant-preview/exit", "POST", {}, z.object({})),
    onSuccess: () => { client.clear(); location.assign("/customers"); },
  });
  if (!session.user.preview) return null;
  const owner = scope.data?.mode === "owner" || session.user.acting;
  return <aside className="border-b border-outline bg-soft px-4 py-3 md:px-7" aria-label="Customer session">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <p role="status" className="text-sm"><strong>{scope.data?.status === "expired" ? "Customer session expired" : owner ? "Acting as customer owner" : "Read-only customer preview"}</strong>{scope.data ? ` · ${scope.data.customerName}` : ""}</p>
      <button className={ui.button("quiet")} disabled={exit.isPending} onClick={() => { exit.mutate(); }}>Exit customer mode</button>
    </div>
    <p className="mt-1 text-xs text-muted">{owner ? "Customer owner permissions apply. Changes are audited as your operator account. API-token creation and public OAuth/MCP delegation are unavailable in this mode." : "You can view this customer; changes are disabled."}</p>
    {scope.error || exit.error ? <p role="alert" className="mt-1 text-sm text-danger">{exit.error?.message ?? scope.error?.message}</p> : null}
  </aside>;
}
