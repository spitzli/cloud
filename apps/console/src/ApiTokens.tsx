import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { z } from "zod";
import { useApi, useSession } from "./context";
import { Empty, Failure, Loading, Modal } from "./common";
import { useTenants } from "./WorkspaceNav";
import { sessionCacheScope } from "./session-cache";
import * as ui from "./ui";

const tokenSchema = z.object({
  id: z.string(),
  name: z.string(),
  canWrite: z.boolean(),
  createdAt: z.string(),
  expiresAt: z.string(),
  lastUsedAt: z.string().nullable(),
  createdBy: z.string(),
});
const createdTokenSchema = tokenSchema.extend({ token: z.string().min(20).max(512) });
const day = (value: string) => new Date(value).toLocaleDateString();

/** A workspace's API tokens: made here, shown once, used as Bearer tokens against the API origin. */
export function WorkspaceApiTokens({ id }: { id: string }) {
  const api = useApi();
  const session = useSession();
  const tenants = useTenants();
  const role = tenants.data?.find((tenant) => tenant.id === id)?.role;
  const manager = !session.user.preview && (session.user.role === "operator" || role === "owner" || role === "admin");
  if (tenants.isPending) return <Loading>Checking workspace access…</Loading>;
  if (tenants.isError) return <Failure error={tenants.error} retry={() => void tenants.refetch()} />;
  if (!manager) return <Empty title="Workspace administrator access required">An owner or administrator manages API tokens. You can continue using the services shared with you.</Empty>;
  const scope = JSON.stringify([api.config.apiOrigin, sessionCacheScope(session), id, role]);
  return <ScopedApiTokens key={scope} id={id} />;
}

function ScopedApiTokens({ id }: { id: string }) {
  const api = useApi();
  const client = useQueryClient();
  const current = useRef(true);
  const request = useRef<AbortController | null>(null);
  const [form, setForm] = useState({ name: "", canWrite: false, days: 90 });
  const [secret, setSecret] = useState<{ id: string; value: string } | null>(null);
  const [copyFeedback, setCopyFeedback] = useState("");
  const [target, setTarget] = useState<z.infer<typeof tokenSchema> | null>(null);
  const [message, setMessage] = useState("");
  useEffect(() => {
    current.current = true;
    return () => { current.current = false; request.current?.abort(); };
  }, []);
  const tokens = useQuery({
    queryKey: ["api-tokens", id],
    queryFn: ({ signal }) => api.get(`/api/console/tenant/${id}/api-tokens`, z.array(tokenSchema), signal),
  });
  const create = useMutation({
    mutationFn: async () => {
      request.current = new AbortController();
      const made = await api.request(`/api/console/tenant/${id}/api-tokens`, createdTokenSchema, { method: "POST", body: JSON.stringify(form), signal: request.current.signal });
      // The one-time secret must never enter React Query's retained mutation data.
      if (current.current) setSecret({ id: made.id, value: made.token });
    },
    gcTime: 0, retry: false,
    onSuccess: () => {
      if (!current.current) return;
      setForm({ name: "", canWrite: false, days: 90 });
      setCopyFeedback(""); setMessage("");
      void client.invalidateQueries({ queryKey: ["api-tokens", id] });
    },
  });
  const revoke = useMutation({
    mutationFn: (token: string) => api.request(`/api/console/tenant/${id}/api-tokens/${token}`, z.object({}), { method: "DELETE" }),
    gcTime: 0, retry: false,
    onSuccess: (_, token) => {
      if (!current.current) return;
      setSecret(value => value?.id === token ? null : value);
      setTarget(null); setMessage("Token revoked. Requests using it will be rejected.");
      void client.invalidateQueries({ queryKey: ["api-tokens", id] });
    },
  });
  const copyToken = async () => {
    if (!secret) return;
    try { await navigator.clipboard.writeText(secret.value); if (current.current) setCopyFeedback("Token copied."); }
    catch { if (current.current) setCopyFeedback("Copy is unavailable. Select the token and copy it manually."); }
  };
  const busy = create.isPending || revoke.isPending;
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>API tokens</h1>
          <p className={ui.pageLead}>Let scripts and CI pipelines use this workspace. A token acts with your role here and nowhere else.</p>
        </div>
        <button type="button" className={ui.button()} disabled={tokens.isFetching} onClick={() => { void tokens.refetch(); }}>{tokens.isFetching ? "Refreshing…" : "Refresh tokens"}</button>
      </div>
      <form
        className={ui.cx(ui.card, "mb-6")}
        onSubmit={(event) => {
          event.preventDefault();
          if (!busy) create.mutate();
        }}
      >
        <fieldset disabled={busy} className={ui.cx(ui.cardBody, "sm:grid-cols-[1fr_auto_auto]")}>
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
        </fieldset>
        <div className={ui.cardFooter}>
          <span role={create.isError ? "alert" : undefined} className={create.isError ? "text-danger" : undefined}>
            {create.isError ? `${create.error.message} Check the token list before creating another token.` : "The token is shown once. Store it in your CI's secrets."}
          </span>
          <button className={ui.button("primary")} disabled={busy}>{create.isPending ? "Creating…" : "Create token"}</button>
        </div>
      </form>
      {message ? <p role="status" className="mb-4 text-sm text-success">{message}</p> : null}
      {secret ? (
        <div role="status" className={ui.cx(ui.notice, "mb-6 grid gap-2")}>
          <strong>Copy this token now. It will not be shown again.</strong>
          <code className={ui.cx(ui.mono, "select-all [overflow-wrap:anywhere]")}>{secret.value}</code>
          <div className="flex flex-wrap items-center gap-3">
            <button type="button" className={ui.button()} onClick={() => { void copyToken(); }}>Copy token</button>
            <button type="button" className={ui.button("quiet")} onClick={() => { setSecret(null); setCopyFeedback(""); }}>Hide token</button>
            <span className="text-sm text-muted">{copyFeedback}</span>
          </div>
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
                <div className="min-w-0 [overflow-wrap:anywhere]">
                  <span className="font-medium">{t.name}</span>
                  <p className="text-xs text-muted">
                    {t.canWrite ? "Read and change" : "Read only"}, by {t.createdBy}, expires {day(t.expiresAt)}, {t.lastUsedAt ? `last used ${day(t.lastUsedAt)}` : "never used"}
                  </p>
                </div>
                <button type="button" className={ui.button("quiet")} disabled={busy} onClick={() => { revoke.reset(); setTarget(t); }}>
                  Revoke
                </button>
              </div>
            ))}
          </div>
        ) : (
          <p className="text-sm text-muted">No tokens yet.</p>
        )
      ) : null}
      {target ? <Modal title="Revoke API token?" close={() => { setTarget(null); }} dismissible={!revoke.isPending}>
        <p className="text-sm [overflow-wrap:anywhere]">Scripts using <strong>{target.name}</strong> will lose access immediately. Create and install a replacement first if they still need access.</p>
        {revoke.isError ? <p role="alert" className="mt-3 text-sm text-danger">{revoke.error.message} Refresh the list to check whether the token was revoked.</p> : null}
        <div className="mt-5 flex flex-wrap justify-end gap-2">
          <button type="button" className={ui.button()} disabled={revoke.isPending} onClick={() => { setTarget(null); }}>Cancel</button>
          <button type="button" className={ui.button("danger")} disabled={revoke.isPending} onClick={() => { revoke.mutate(target.id); }}>{revoke.isPending ? "Revoking…" : "Revoke token"}</button>
        </div>
      </Modal> : null}
    </section>
  );
}
