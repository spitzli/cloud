// Host-supplied UI ABI; the independent plugin owns all Neon presentation and action contracts.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { z } from "zod";
import { Failure, Loading, ui, useI18n, usePluginQuery, usePluginReveal, usePluginSubmission, type CreateProps, type PluginUI, type ResourceProps } from "@webdock/plugin-ui";

const item = z.object({ id: z.string(), name: z.string() });
const inventorySchema = z.object({
  projects: z.array(item.extend({ region_id: z.string().optional() })),
  branches: z.array(item),
  databases: z.array(z.object({ name: z.string() })),
  roles: z.array(z.object({ name: z.string() })),
  endpoints: z.array(z.object({ id: z.string(), branch_id: z.string(), type: z.string().optional(), current_state: z.string().optional() })),
  cursor: z.string().nullable(),
});
const attachedSchema = z.object({ resource: item });
const detachedSchema = z.object({ detached: z.literal(true), resource_id: z.string() });
const secretSchema = z.object({ value: z.string().max(8000) }).strict();
const panelSchema = z.object({ title: z.string(), blocks: z.array(z.object({
  kind: z.string(), title: z.string().optional(), text: z.string().optional(),
  columns: z.array(z.object({ key: z.string(), label: z.string() })).optional(),
  rows: z.array(z.record(z.string(), z.union([z.string(), z.boolean(), z.null()]))).optional(),
})) });

// Translate only plugin-owned metadata; resource names and table values remain customer data.
function panelText(text: string | undefined, t: ReturnType<typeof useI18n>["t"]) {
  const labels: Record<string, string> = {
    "Connection settings": t("Connection settings"),
    "Setting": t("Setting"),
    "Value": t("Value"),
    "Compute state": t("Compute state"),
    "Neon project": t("Neon project"),
    "Branch": t("Branch"),
    "Database": t("Database"),
    "Role": t("Role"),
    "Endpoint": t("Endpoint"),
    "SSL mode": t("SSL mode"),
    "Branches": t("Branches"),
    "Neon branches": t("Neon branches"),
    "ID": t("ID"),
    "State": t("State"),
    "Compute": t("Compute"),
    "Neon compute is active": t("Neon compute is active"),
    "Neon compute is idle; a database connection can wake it": t("Neon compute is idle; a database connection can wake it"),
    "Neon compute is starting": t("Neon compute is starting"),
    "Neon compute state is not recognized": t("Neon compute state is not recognized"),
  };
  return text === undefined ? undefined : Object.hasOwn(labels, text) ? labels[text] : text;
}

function Connections({ customer }: { customer: string }) {
  const { t } = useI18n();
  return <a className="text-sm underline" href={`/workspaces/${encodeURIComponent(customer)}/connections`}>{t("Workspace Connections")}</a>;
}
function Select({ label, placeholder, value, onChange, children, disabled = false }: { label: string; placeholder: string; value: string; onChange: (value: string) => void; children: ReactNode; disabled?: boolean }) {
  return <label className={ui.field}>{label}<select className={ui.input} value={value} onChange={event => { onChange(event.target.value); }} disabled={disabled}><option value="">{placeholder}</option>{children}</select></label>;
}
function Create(context: CreateProps) {
  const { t } = useI18n();
  const [project, setProject] = useState("");
  const [branch, setBranch] = useState("");
  const [database, setDatabase] = useState("");
  const [role, setRole] = useState("");
  const [endpoint, setEndpoint] = useState("");
  const [name, setName] = useState("");
  const [cursor, setCursor] = useState<string | undefined>();
  const inventory = usePluginQuery(context, "databases.discover", {
    ...(project ? { neon_project_id: project } : {}), ...(branch ? { branch_id: branch } : {}), ...(cursor ? { cursor } : {}),
  }, inventorySchema);
  const attach = usePluginSubmission(context, context.action, attachedSchema);
  const submission = useRef<{ canonical: string; idempotencyKey: string } | null>(null);
  const active = useRef<AbortController | null>(null);
  useEffect(() => () => active.current?.abort(), []);
  const clearTuple = () => { setDatabase(""); setRole(""); setEndpoint(""); };
  const data = inventory.data;
  const locked = attach.isPending || inventory.isPending;
  const validTuple = data?.databases.some(row => row.name === database) && data.roles.some(row => row.name === role) && data.endpoints.some(row => row.id === endpoint && row.branch_id === branch);
  const ready = Boolean(name.trim() && project && branch && validTuple && !locked && !inventory.error);
  return <form className="grid gap-5" onSubmit={event => {
    event.preventDefault();
    if (!ready) return;
    const input = { name: name.trim(), neon_project_id: project, branch_id: branch, database_name: database, role_name: role, endpoint_id: endpoint };
    const canonical = JSON.stringify(input);
    if (submission.current?.canonical !== canonical) submission.current = { canonical, idempotencyKey: crypto.randomUUID() };
    const controller = new AbortController(); active.current = controller;
    void attach.mutateAsync({ input, idempotencyKey: submission.current.idempotencyKey, signal: controller.signal }).then(result => { context.onCreated(result.resource.id); }).catch(() => undefined);
  }}>
    <p className="text-sm text-muted">{t("Connect an existing database. Your Neon account keeps ownership and billing. Your API key is stored encrypted.")}</p>
    <Connections customer={context.customer} />
    <p className="text-sm text-muted">{t("Choose a Neon project in Frankfurt, Germany. Other regions are not enabled for database attachments in this release.")}</p>
    <label className={ui.field}>{t("Display name")}<input className={ui.input} value={name} onChange={event => { setName(event.target.value); }} maxLength={80} required disabled={attach.isPending} /></label>
    <Select label={t("Neon project")} placeholder={t("Select neon project…")} value={project} disabled={locked} onChange={value => { setProject(value); setBranch(""); clearTuple(); setCursor(undefined); }}>
      {data?.projects.map(row => <option key={row.id} value={row.id} disabled={row.region_id !== "aws-eu-central-1"}>{row.name}{row.region_id ? ` · ${row.region_id}` : ""}</option>)}
      {project && !data?.projects.some(row => row.id === project) ? <option value={project}>{t("Selected project")}</option> : null}
    </Select>
    {project ? <Select label={t("Branch")} placeholder={t("Select branch…")} value={branch} disabled={locked} onChange={value => { setBranch(value); clearTuple(); setCursor(undefined); }}>
      {data?.branches.map(row => <option key={row.id} value={row.id}>{row.name}</option>)}
      {branch && !data?.branches.some(row => row.id === branch) ? <option value={branch}>{t("Selected branch")}</option> : null}
    </Select> : null}
    {data?.cursor && !branch ? <button type="button" className={ui.button("quiet")} disabled={attach.isPending} onClick={() => { setCursor(data.cursor ?? undefined); }}>{project ? t("Next branches") : t("Next projects")}</button> : null}
    {cursor ? <button type="button" className={ui.button("quiet")} disabled={attach.isPending} onClick={() => { setCursor(undefined); }}>{t("First page")}</button> : null}
    {branch ? <>
      <Select label={t("Database")} placeholder={t("Select database…")} value={database} disabled={locked} onChange={setDatabase}>{data?.databases.map(row => <option key={row.name} value={row.name}>{row.name}</option>)}</Select>
      <Select label={t("Role")} placeholder={t("Select role…")} value={role} disabled={locked} onChange={setRole}>{data?.roles.map(row => <option key={row.name} value={row.name}>{row.name}</option>)}</Select>
      <Select label={t("Endpoint")} placeholder={t("Select endpoint…")} value={endpoint} disabled={locked} onChange={setEndpoint}>{data?.endpoints.map(row => <option key={row.id} value={row.id}>{row.type ?? t("Compute")} · {row.current_state ?? t("unknown")} · {row.id}</option>)}</Select>
      {data && !data.endpoints.length ? <p role="status" className="text-sm text-muted">{t("This branch has no compute endpoint. Choose another branch or configure an endpoint in Neon.")}</p> : null}
    </> : null}
    {inventory.isPending ? <Loading>{t("Loading Neon inventory…")}</Loading> : null}
    {inventory.error ? <div role="alert"><p>{t("Could not load Neon inventory. Check your connection and permissions.")}</p><button type="button" className={ui.button("quiet")} onClick={() => void inventory.refetch()}>{t("Retry")}</button></div> : null}
    {data && !data.projects.length && !project ? <p className="text-sm text-muted">{t("No authorized Neon projects found. Check the workspace connection or create a project in Neon.")}</p> : null}
    {attach.error ? <p role="alert" className="text-sm text-danger">{t("Could not attach this database. You can retry the same selection.")}</p> : null}
    <div className="flex flex-wrap gap-3"><button type="submit" className={ui.button()} disabled={!ready}>{attach.isPending ? t("Connecting…") : t("Connect database")}</button><button type="button" className={ui.button("quiet")} disabled={attach.isPending} onClick={context.onCancel}>{t("Cancel")}</button></div>
  </form>;
}
function Table({ panel }: { panel: z.infer<typeof panelSchema> }) {
  const { t } = useI18n();
  return <>{panel.blocks.filter(block => block.kind === "table").map((block, index) => <section className={ui.card} key={index} aria-label={panelText(block.title, t)}>
    <div className={ui.cardBody}><h2 className={ui.sectionTitle}>{panelText(block.title, t)}</h2><div className="overflow-x-auto"><table className="w-full text-left text-sm"><thead><tr>{block.columns?.map(column => <th className="py-2 pr-4" key={column.key}>{panelText(column.label, t)}</th>)}</tr></thead><tbody>{block.rows?.map((row, rowIndex) => <tr key={rowIndex}>{block.columns?.map(column => <td className={ui.cx("align-top border-t border-outline py-2 pr-4", column.key === "field" ? "whitespace-nowrap" : "break-all")} key={column.key}>{column.key === "field" ? panelText(String(row[column.key] ?? "—"), t) : String(row[column.key] ?? "—")}</td>)}</tr>)}</tbody></table></div>{!block.rows?.length ? <p className="text-sm text-muted">{t("No branches found.")}</p> : null}</div>
  </section>)}</>;
}
function Credentials({ context }: { context: ResourceProps }) {
  const { t } = useI18n();
  const reveal = usePluginReveal(context, "databases.credentials");
  const [pooled, setPooled] = useState(true);
  const [copied, setCopied] = useState(false);
  const secret = secretSchema.safeParse(reveal.data);
  const visible = secret.success;
  const reset = useRef(reveal.reset);
  useEffect(() => { reset.current = reveal.reset; }, [reveal.reset]);
  useEffect(() => {
    // Keep the listener while pending too: hiding aborts a late secret response.
    const hide = () => { if (document.hidden) { reset.current(); setCopied(false); } };
    document.addEventListener("visibilitychange", hide);
    hide();
    return () => { document.removeEventListener("visibilitychange", hide); };
  }, []);
  useEffect(() => {
    if (!visible) return;
    if (document.hidden) { reset.current(); return; }
    // Copy and unrelated renders must not extend this disclosure deadline.
    const timeout = window.setTimeout(() => { reset.current(); }, 60_000);
    return () => { window.clearTimeout(timeout); };
  }, [visible]);
  return <section className={ui.card} aria-label={t("Connection URI")}><div className={ui.cardBody}>
    <h2 className={ui.sectionTitle}>{t("Connection URI")}</h2><p className="text-sm text-muted">{t("Reveal requires write access. The URI is hidden after one minute or when you leave this tab.")}</p>
    <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={pooled} onChange={event => { reveal.reset(); setCopied(false); setPooled(event.target.checked); }} />{t("Use pooled connection")}</label>
    {secret.success ? <><code className="block break-all rounded border border-outline p-3 text-sm">{secret.data.value}</code><div className="flex flex-wrap gap-2"><button type="button" className={ui.button("quiet")} onClick={() => { void navigator.clipboard.writeText(secret.data.value).then(() => { setCopied(true); }).catch(() => { setCopied(false); }); }}>{copied ? t("Copied") : t("Copy URI")}</button><button type="button" className={ui.button("quiet")} onClick={() => { reveal.reset(); setCopied(false); }}>{t("Hide URI")}</button></div></> : <button type="button" className={ui.button()} disabled={reveal.isPending} onClick={() => { setCopied(false); reveal.mutate({ resource_id: context.resource.id, pooled }); }}>{reveal.isPending ? t("Loading…") : t("Reveal URI")}</button>}
    {reveal.error || (reveal.isSuccess && !secret.success) ? <p role="alert" className="text-sm text-danger">{t("Could not reveal the connection URI. Check your access and try again.")}</p> : null}
  </div></section>;
}
function BranchInventory({ context }: { context: ResourceProps }) {
  const { t } = useI18n();
  const branchView = usePluginQuery(context, "databases.branches", { resource_id: context.resource.id }, panelSchema);
  return branchView.data ? <Table panel={branchView.data} /> : branchView.error ? <Failure error={branchView.error} retry={() => { void branchView.refetch(); }} /> : <Loading>{t("Loading branches…")}</Loading>;
}
function Resource(context: ResourceProps) {
  const { t } = useI18n();
  const view = usePluginQuery(context, "databases.view", { resource_id: context.resource.id }, panelSchema);
  const detach = usePluginSubmission(context, "databases.detach", detachedSchema);
  const key = useRef(crypto.randomUUID());
  const [confirming, setConfirming] = useState(false);
  const canReveal = context.writable && (!context.actions || context.actions.includes("databases.credentials"));
  const canDetach = context.writable && (!context.actions || context.actions.includes("databases.detach"));
  return <div className="grid gap-6">
    <section className={ui.card}><div className={ui.cardBody}><h2 className={ui.sectionTitle}>{view.data?.title ?? context.resource.name}</h2><p className="text-sm text-muted">{panelText(view.data?.blocks.find(block => block.kind === "status")?.text, t)}</p>{context.writable ? <Connections customer={context.customer} /> : null}</div></section>
    {view.isPending ? <Loading>{t("Loading Neon database…")}</Loading> : view.error ? <Failure error={view.error} retry={() => void view.refetch()} /> : <Table panel={view.data} />}
    {context.writable ? <BranchInventory context={context} /> : null}
    {canReveal && view.data && !view.error ? <Credentials key={context.resource.id} context={context} /> : null}
    {canDetach ? <section className={ui.card}><div className={ui.cardBody}><h2 className={ui.sectionTitle}>{t("Detach database")}</h2><p className="text-sm text-muted">{t("Remove the attachment from this Spitzli Cloud project. The database, branches and all Neon data remain in your Neon account.")}</p>{confirming ? <div className="flex flex-wrap gap-3"><button type="button" className={ui.cx(ui.button("danger"), "whitespace-normal py-2")} disabled={detach.isPending} onClick={() => { void detach.mutateAsync({ input: { resource_id: context.resource.id }, idempotencyKey: key.current }).then(context.onDeleted).catch(() => undefined); }}>{detach.isPending ? t("Detaching…") : t("Confirm detach")}</button><button type="button" className={ui.button("quiet")} disabled={detach.isPending} onClick={() => { setConfirming(false); }}>{t("Cancel")}</button></div> : <button type="button" className={ui.cx(ui.button("danger"), "whitespace-normal py-2")} onClick={() => { setConfirming(true); }}>{t("Detach database")}</button>}{detach.error ? <p role="alert" className="text-sm text-danger">{t("Could not detach. Retry to safely resume this request.")}</p> : null}</div></section> : null}
  </div>;
}
export default { Resource, Create: { "databases.attach": Create } } satisfies PluginUI;
