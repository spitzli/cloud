import { useCallback, useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { useI18n } from "@spitzli/i18n/react";
import { z } from "zod";
import { ApiError } from "./api";
import { useApi, useSession } from "./context";
import { sessionCacheScope } from "./session-cache";
import { Empty, Failure, Loading } from "./common";
import * as ui from "./ui";

const resultSchema = z.object({ data: z.object({
  canManage: z.boolean(),
  connection: z.object({ id: z.string(), revision: z.number().int().nonnegative(), status: z.enum(["connected", "disconnected"]), authMethod: z.literal("api-key"), organizationID: z.string().nullable(), neonProjectID: z.string().nullable(), connectedAt: z.string() }).nullable(),
  projects: z.array(z.object({ id: z.string(), name: z.string(), regionId: z.string() })).max(100).optional(),
  nextCursor: z.string().nullable().optional(),
}) });
interface Intent { action: "connect" | "disconnect"; expectedRevision: number; idempotencyKey: string; organizationID?: string; neonProjectID?: string; digest?: string }
const uncertain = (error: unknown) => !(error instanceof ApiError) || error.status >= 500;
const fingerprint = async (value: string) => Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value))), byte => byte.toString(16).padStart(2, "0")).join("");

export function WorkspaceConnections({ id }: { id: string }) {
  const api = useApi(), session = useSession();
  const scope = JSON.stringify([api.config.apiOrigin, sessionCacheScope(session), id]);
  return <ScopedConnections key={scope} customerID={id} scope={scope} />;
}

function ScopedConnections({ customerID, scope }: { customerID: string; scope: string }) {
  const { t } = useI18n();
  const api = useApi(), session = useSession(), client = useQueryClient();
  const current = useRef(true), secret = useRef("");
  const input = useRef<HTMLInputElement>(null);
  const setSecretInput = useCallback((node: HTMLInputElement | null) => {
    if (input.current && input.current !== node) input.current.value = "";
    input.current = node;
  }, []);
  const intent = useRef<Intent | null>(null);
  const [message, setMessage] = useState("");
  const [retrying, setRetrying] = useState<"connect" | "disconnect" | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const [cursor, setCursor] = useState<string | undefined>();
  const [inventoryOpen, setInventoryOpen] = useState(false);
  const call = async (command: object) => (await api.write("/api/console/byok/neon", "POST", { ...command, customerID }, resultSchema)).data;
  const queryKey = ["neon-connection", scope];
  useEffect(() => {
    current.current = true;
    return () => {
      current.current = false; secret.current = ""; intent.current = null;
      void client.cancelQueries({ queryKey: ["neon-connection", scope] });
      client.removeQueries({ queryKey: ["neon-connection", scope] });
    };
  }, [client, scope]);
  const metadata = useQuery({ queryKey, queryFn: () => call({ action: "get" }), retry: false, refetchOnWindowFocus: false });
  const canManage = metadata.data?.canManage === true && !(session.user.preview && !session.user.acting);
  const connection = metadata.data?.connection;
  const connected = connection?.status === "connected";
  const inventory = useQuery({
    queryKey: [...queryKey, "projects", connection?.revision, cursor],
    queryFn: async () => {
      try { return await call({ action: "projects", ...(cursor ? { cursor } : {}) }); }
      catch (error) {
        if (current.current && error instanceof ApiError && (error.status === 409 || error.status === 403)) {
          setInventoryOpen(false);
          setMessage(t("The connection changed. Review the refreshed status before submitting a new request."));
          void metadata.refetch();
        }
        throw error;
      }
    },
    enabled: canManage && connected && inventoryOpen,
    retry: false, refetchOnWindowFocus: false, refetchOnReconnect: false,
  });
  const change = useMutation({
    // Mutation variables and cached results must never retain the API key.
    mutationFn: async () => {
      const command = intent.current;
      const apiKey = secret.current; secret.current = "";
      if (!command || !canManage) throw new Error("Connection access changed.");
      const publicCommand = { action: command.action, expectedRevision: command.expectedRevision, idempotencyKey: command.idempotencyKey, ...(command.organizationID ? { organizationID: command.organizationID } : {}), ...(command.neonProjectID ? { neonProjectID: command.neonProjectID } : {}) };
      const data = await call({ ...publicCommand, ...(command.action === "connect" ? { apiKey } : {}) });
      if (data.connection?.revision !== command.expectedRevision + 1 || data.connection.status !== (command.action === "connect" ? "connected" : "disconnected")) throw new Error("Unconfirmed connection receipt.");
      if (command.action === "connect" && data.connection.neonProjectID !== (command.neonProjectID ?? null)) throw new Error("Unconfirmed Neon project scope.");
      return data;
    },
    gcTime: 0, retry: false,
    onSuccess: data => {
      if (!current.current) return;
      client.setQueryData(queryKey, data);
      setInventoryOpen(false); setCursor(undefined);
      intent.current = null; setRetrying(null); setConfirm(false);
      setMessage(data.connection?.status === "connected" ? t("Neon connected. Refresh projects to read your Neon inventory.") : t("Neon disconnected. Your Neon projects and data are unchanged."));
    },
    onError: error => {
      if (!current.current) return;
      secret.current = "";
      setInventoryOpen(false);
      if (uncertain(error)) {
        setRetrying(intent.current?.action ?? null);
        setMessage(t("The result is uncertain. Retry the same request. For a connection, re-enter the same API key; the original revision and request ID are retained."));
      } else {
        intent.current = null; setRetrying(null); setConfirm(false);
        setMessage(error instanceof ApiError && error.status === 409 ? t("The connection changed. Review the refreshed status before submitting a new request.") : t("Neon rejected this request or access is unavailable. Check your API key, Neon project ID and permissions. No alternative provider was used."));
        void metadata.refetch();
      }
    },
  });
  const busy = preparing || change.isPending || metadata.isFetching;
  async function connect(event: React.SyntheticEvent<HTMLFormElement>) {
    event.preventDefault();
    const apiKey = input.current?.value ?? "";
    const organizationInput = event.currentTarget.elements.namedItem("organizationID") as HTMLInputElement | null;
    const projectInput = event.currentTarget.elements.namedItem("neonProjectID") as HTMLInputElement | null;
    const organizationID = organizationInput?.value.trim() ?? "";
    const neonProjectID = projectInput?.value.trim() ?? "";
    event.currentTarget.reset();
    if (organizationInput) organizationInput.value = organizationID;
    if (projectInput) projectInput.value = neonProjectID;
    if (!canManage || busy || !apiKey) return;
    setPreparing(true); setMessage("");
    try {
      const digest = await fingerprint(JSON.stringify([apiKey, organizationID, neonProjectID]));
      if (!current.current) return;
      if (intent.current && (intent.current.action !== "connect" || intent.current.digest !== digest)) {
        setMessage(t("Re-enter the same API key and keep the original organization and Neon project IDs to retry the uncertain request.")); return;
      }
      intent.current ??= { action: "connect", expectedRevision: connection?.revision ?? 0, idempotencyKey: crypto.randomUUID(), ...(organizationID ? { organizationID } : {}), ...(neonProjectID ? { neonProjectID } : {}), digest };
      secret.current = apiKey;
      change.mutate();
    } catch {
      setMessage(t("The API key could not be prepared. Enter it again to retry."));
    } finally { if (current.current) setPreparing(false); }
  }
  function disconnect() {
    if (!canManage || busy || !confirm) return;
    intent.current ??= { action: "disconnect", expectedRevision: connection?.revision ?? 0, idempotencyKey: crypto.randomUUID() };
    if (intent.current.action === "disconnect") change.mutate();
  }
  if (metadata.isPending) return <Loading>{t("Loading connections…")}</Loading>;
  if (metadata.isError) return metadata.error instanceof ApiError && metadata.error.status === 403
    ? <Empty title={t("Workspace administrator access required")}>{t("An owner, administrator or authorized operator manages Neon connections.")}</Empty>
    : <Failure error={metadata.error} retry={() => { void metadata.refetch(); }} />;
  return <section className="min-w-0">
    <div className={ui.pageHeading}><div><h1 className={ui.pageTitle}>{t("Connections")}</h1><p className={ui.pageLead}>{t("Connect your own Neon account to this customer workspace.")}</p></div></div>
    <div className={ui.card}>
      <div className={ui.cardBody}>
        <div className="flex flex-wrap items-center justify-between gap-3"><h2 className={ui.sectionTitle}>Neon</h2><span className={ui.badge(connected ? "success" : "neutral")}>{connected ? t("Connected") : t("Disconnected")}</span></div>
        <p className="text-sm text-muted">{t("Neon bills your own account and applies its own quotas. This connection does not use the platform's free database allowance.")}</p>
        {connection ? <dl className="grid gap-2 text-sm sm:grid-cols-2"><div><dt className="text-muted">{t("Organization")}</dt><dd className="[overflow-wrap:anywhere]">{connection.organizationID ?? t("API key scope")}</dd></div><div><dt className="text-muted">{t("Connection revision")}</dt><dd>{connection.revision}</dd></div></dl> : null}
        {connection?.neonProjectID ? <p className="text-sm [overflow-wrap:anywhere]">{t("Neon project scope")}: <span className={ui.mono}>{connection.neonProjectID}</span></p> : null}
        {!canManage ? <p className={ui.notice}>{t("Read-only view. Connection changes and provider inventory refresh are disabled.")}</p> : null}
        {message ? <p role={change.isError ? "alert" : "status"} className={ui.notice}>{message}</p> : null}
        {canManage ? <>
          {(retrying !== "disconnect") ? <form aria-label={t("Connect Neon")} autoComplete="off" className="grid gap-4" onSubmit={event => { void connect(event); }}>
            <label className={ui.field}>{t("Neon API key")}<input ref={setSecretInput} name="neon-api-key" type="password" autoComplete="new-password" data-lpignore="true" spellCheck={false} autoCapitalize="none" required maxLength={8192} disabled={busy} className={ui.input} aria-describedby="neon-key-help" /></label>
            <p id="neon-key-help" className={ui.hint}>{t("Create a key in your Neon account. The key is stored encrypted on the server and is never shown again. It is cleared from this form on every submission.")}</p>
            <label className={ui.field}>{t("Organization ID (optional)")}<input name="organizationID" className={ui.input} maxLength={200} autoComplete="off" disabled={busy || !!retrying} /></label>
            <label className={ui.field}>{t("Neon project ID (optional)")}<input name="neonProjectID" className={ui.input} maxLength={200} autoComplete="off" disabled={busy || !!retrying} aria-describedby="neon-project-help" /></label>
            <p id="neon-project-help" className={ui.hint}>{t("Recommended for project-scoped API keys: enter the exact Neon project ID. Leave blank for account or organization keys to list their available projects.")}</p>
            <div><button className={ui.button("primary")} disabled={busy}>{busy ? t("Connecting…") : retrying ? t("Retry same connection") : connected ? t("Replace connection key") : t("Connect Neon")}</button></div>
          </form> : null}
          <details><summary className="cursor-pointer text-sm font-medium">{t("Optional: create a project key with the Neon CLI")}</summary><div className="mt-3 grid gap-3 text-sm text-muted"><p>{t("Run the official Neon CLI on your own computer. The localhost login callback belongs to that same computer. Use that same project ID in the Neon project ID field above, then paste only the project API key into the form.")}</p><pre className="overflow-x-auto rounded bg-subtle p-3 text-xs"><code>{"neon login\nneon api-keys create --project-id <selected-id> --name Spitzli-Cloud"}</code></pre><p>{t("Do not upload credentials.json or share a login callback URL or code. Spitzli does not run the CLI for you.")}</p></div></details>
        </> : null}
      </div>
      {canManage && connected ? <div className={ui.cardFooter}>
        <a href="https://console.neon.tech" target="_blank" rel="noopener noreferrer">{t("Open Neon Console")}</a>
        {!confirm ? <button className={ui.button()} disabled={busy || !!retrying} onClick={() => { setConfirm(true); }}>{t("Disconnect Neon")}</button> : <div role="group" aria-label={t("Confirm Neon disconnection")} className="grid gap-3"><p>{t("Remove only the Spitzli connection and stored API key? Your Neon projects and data remain intact.")}</p><div className="flex flex-wrap gap-2"><button className={ui.button("danger")} disabled={busy} onClick={disconnect}>{retrying ? t("Retry same disconnection") : t("Confirm disconnect")}</button><button className={ui.button()} disabled={busy || !!retrying} onClick={() => { setConfirm(false); }}>{t("Cancel")}</button></div></div>}
      </div> : null}
    </div>
    {canManage && connected ? <NeonInventory inventory={inventory} inventoryOpen={inventoryOpen} disabled={busy || !!retrying} refresh={() => { if (!inventoryOpen) { setInventoryOpen(true); return; } if (cursor) { setCursor(undefined); return; } void inventory.refetch(); }} next={setCursor} /> : null}
  </section>;
}

function NeonInventory({ inventory, inventoryOpen, disabled, refresh, next }: { inventory: UseQueryResult<z.infer<typeof resultSchema>["data"]>; inventoryOpen: boolean; disabled: boolean; refresh: () => void; next: (cursor: string | undefined) => void }) {
  const { t } = useI18n();
  return <div className="mt-6 grid gap-4" aria-label={t("Neon project inventory")}>
      <div className="flex flex-wrap items-center justify-between gap-3"><h2 className={ui.sectionTitle}>{t("Neon projects (read only)")}</h2><button className={ui.button()} disabled={disabled || inventory.isFetching} onClick={refresh}>{t("Refresh projects")}</button></div>
      {!inventoryOpen ? <p className="text-sm text-muted">{t("Refresh to read projects available to this key. No database resources are provisioned by connecting.")}</p> : inventory.isPending ? <Loading>{t("Reading Neon projects…")}</Loading> : inventory.isError ? <p role="alert" className={ui.notice}>{t("Neon inventory is unavailable. The key may be invalid or revoked, or Neon may be unavailable. Check the key and refresh; no fallback is used.")}</p> : <>
        <ul className={ui.card}>{inventory.data.projects?.length ? inventory.data.projects.map(project => <li key={project.id} className={ui.cx(ui.cardRow, "first:border-t-0")}><div className="min-w-0 [overflow-wrap:anywhere]"><strong>{project.name}</strong><p className={ui.mono}>{project.id}</p></div><span className="text-muted [overflow-wrap:anywhere]">{project.regionId}</span></li>) : <li className={ui.cardBody}>{t("No Neon projects are available to this key.")}</li>}</ul>
        {inventory.data.nextCursor ? <div><button className={ui.button()} disabled={inventory.isFetching} onClick={() => { next(inventory.data.nextCursor ?? undefined); }}>{t("Next project page")}</button></div> : null}
      </>}
    </div>;
}
