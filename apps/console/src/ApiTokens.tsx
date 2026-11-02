import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import { useApi, useSession } from "./context";
import { Empty, Failure, Loading } from "./common";
import { useTenants } from "./WorkspaceNav";
import * as ui from "./ui";

const tokenSchema = z.object({
  id: z.string(),
  name: z.string(),
  canWrite: z.boolean(),
  createdAt: z.string(),
  expiresAt: z.string(),
  lastUsedAt: z.string().nullable(),
  createdBy: z.string(),
  token: z.string().optional(),
});
const day = (value: string) => new Date(value).toLocaleDateString();

/** A workspace's API tokens: made here, shown once, used as Bearer tokens against the API origin. */
export function WorkspaceApiTokens({ id }: { id: string }) {
  const api = useApi();
  const session = useSession();
  const tenants = useTenants();
  const role = tenants.data?.find((tenant) => tenant.id === id)?.role;
  const manager = !session.user.preview && (session.user.role === "operator" || role === "owner" || role === "admin");
  const client = useQueryClient();
  const [form, setForm] = useState({ name: "", canWrite: false, days: 90 });
  const tokens = useQuery({
    queryKey: ["api-tokens", id],
    enabled: manager,
    queryFn: ({ signal }) => api.get(`/api/console/tenant/${id}/api-tokens`, z.array(tokenSchema), signal),
  });
  const create = useMutation({
    mutationFn: () => api.write(`/api/console/tenant/${id}/api-tokens`, "POST", form, tokenSchema),
    onSuccess: () => {
      setForm({ name: "", canWrite: false, days: 90 });
      void client.invalidateQueries({ queryKey: ["api-tokens", id] });
    },
  });
  const revoke = useMutation({
    mutationFn: (token: string) => api.request(`/api/console/tenant/${id}/api-tokens/${token}`, z.object({}), { method: "DELETE" }),
    onSuccess: () => void client.invalidateQueries({ queryKey: ["api-tokens", id] }),
  });
  if (tenants.isPending) return <Loading>Checking workspace access…</Loading>;
  if (tenants.isError) return <Failure error={tenants.error} retry={() => void tenants.refetch()} />;
  if (!manager) return <Empty title="Workspace administrator access required">An owner or administrator manages API tokens. You can continue using the services shared with you.</Empty>;
  const secret = create.data?.token;
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>API tokens</h1>
          <p className={ui.pageLead}>Let scripts and CI pipelines use this workspace. A token acts with your role here and nowhere else.</p>
        </div>
      </div>
      <form
        className={ui.cx(ui.card, "mb-6")}
        onSubmit={(event) => {
          event.preventDefault();
          create.mutate();
        }}
      >
        <div className={ui.cx(ui.cardBody, "sm:grid-cols-[1fr_auto_auto]")}>
          <label className={ui.field}>
            Name
            <input className={ui.input} required maxLength={80} value={form.name} onChange={(e) => { setForm({ ...form, name: e.target.value }); }} placeholder="CI deploy" />
          </label>
          <label className={ui.field}>
            Access
            <select className={ui.input} value={form.canWrite ? "write" : "read"} onChange={(e) => { setForm({ ...form, canWrite: e.target.value === "write" }); }}>
              <option value="read">Read only</option>
              <option value="write">Read and change</option>
            </select>
          </label>
          <label className={ui.field}>
            Expires after
            <select className={ui.input} value={form.days} onChange={(e) => { setForm({ ...form, days: Number(e.target.value) }); }}>
              <option value={30}>30 days</option>
              <option value={90}>90 days</option>
              <option value={365}>1 year</option>
            </select>
          </label>
        </div>
        <div className={ui.cardFooter}>
          <span role={create.isError ? "alert" : undefined} className={create.isError ? "text-danger" : undefined}>
            {create.isError ? create.error.message : "The token is shown once. Store it in your CI's secrets."}
          </span>
          <button className={ui.button("primary")} disabled={create.isPending}>Create token</button>
        </div>
      </form>
      {secret ? (
        <div role="status" className={ui.cx(ui.notice, "mb-6 grid gap-2")}>
          <strong>Copy this token now. It will not be shown again.</strong>
          <code className={ui.cx(ui.mono, "select-all [overflow-wrap:anywhere]")}>{secret}</code>
          <span className="text-muted">
            Use it as <code className={ui.mono}>Authorization: Bearer …</code> against <code className={ui.mono}>{new URL("/v1/api/projects", api.config.workspaceApiOrigin ?? api.config.apiOrigin).href}</code>.
          </span>
        </div>
      ) : null}
      {tokens.isPending ? <Loading>Loading tokens…</Loading> : null}
      {tokens.isError ? <Failure error={tokens.error} retry={() => void tokens.refetch()} /> : null}
      {tokens.data ? (
        tokens.data.length ? (
          <div className={ui.card}>
            {tokens.data.map((t) => (
              <div key={t.id} className={ui.cardRow}>
                <div className="min-w-0">
                  <span className="font-medium">{t.name}</span>
                  <p className="text-xs text-muted">
                    {t.canWrite ? "Read and change" : "Read only"}, by {t.createdBy}, expires {day(t.expiresAt)}, {t.lastUsedAt ? `last used ${day(t.lastUsedAt)}` : "never used"}
                  </p>
                </div>
                <button type="button" className={ui.button("quiet")} disabled={revoke.isPending} onClick={() => { revoke.mutate(t.id); }}>
                  Revoke
                </button>
              </div>
            ))}
          </div>
        ) : (
          <p className="text-sm text-muted">No tokens yet.</p>
        )
      ) : null}
      {revoke.isError ? <p role="alert" className="mt-3 text-sm text-danger">{revoke.error.message}</p> : null}
    </section>
  );
}
