// Read/status controls use the native SDK. Secret reveal remains an explicit synchronous API action.
import { useEffect, useRef, useState } from "react";
import { z } from "zod";
import { Failure, Loading, ui, usePluginQuery, usePluginReveal, usePluginSubmission, usePluginStorage, usePluginScope, storageKeySchema, storageHashSchema, STORAGE_CONTENT_TYPES, type PluginUI, type ResourceProps, type CreateProps } from "@webdock/plugin-ui";

const viewSchema = z.object({
  bucket: z.string(), endpoint: z.string(), environment: z.string(), revision: z.number(), state: z.string(),
  credentials: z.array(z.string()),
  observed: z.object({ observed_at: z.number(), exists: z.boolean(), owned: z.boolean(), versioning: z.boolean() }).nullable(),
  inventory: z.object({ current_bytes: z.number(), noncurrent_bytes: z.number(), multipart_bytes: z.number(), complete: z.boolean(), observed_at: z.number() }).nullable(),
});
type View = z.infer<typeof viewSchema>;
const result = z.unknown();
function Change({ context, view, action, label, extra = {}, destructive = false }: {
  context: ResourceProps; view: View; action: string; label: string; extra?: Record<string, unknown>; destructive?: boolean;
}) {
  // The action can change after a refresh (create → rotate); freeze the whole attempt.
  const [submission, setSubmission] = useState<{ action: string; label: string; input: Record<string, unknown>; idempotencyKey: string }>();
  const mutation = usePluginSubmission(context, submission?.action ?? action, result);
  return <div>
    <button type="button" className={ui.button(destructive ? "danger" : "secondary")} disabled={mutation.isPending} onClick={() => {
      const next = submission ?? { action, label, input: { resource_id: context.resource.id, request_id: crypto.randomUUID(), revision: view.revision, ...extra }, idempotencyKey: crypto.randomUUID() };
      setSubmission(next);
      mutation.mutate(next, { onSuccess: () => { setSubmission(undefined); if (next.action === "buckets.delete") context.onDeleted(); } });
    }}>{mutation.isPending ? "Working…" : mutation.error ? `Retry ${(submission?.label ?? label).toLowerCase()}` : label}</button>
    {mutation.error ? <><p role="alert" className="text-sm text-danger">{mutation.error.message} Refresh the status before attempting another operation.</p><button type="button" className={ui.button("quiet")} onClick={() => { setSubmission(undefined); mutation.reset(); }}>Discard retry</button></> : null}
  </div>;
}
const credentialSchema = z.object({ access_key: z.string(), secret_key: z.string(), endpoint: z.string(), bucket: z.string() });
function CredentialReveal({ context, accessKey }: { context: ResourceProps; accessKey: string }) {
  const reveal = usePluginReveal(context, "credentials.reveal");
  const [copied, setCopied] = useState(false);
  const parsed = credentialSchema.safeParse(reveal.data);
  const credential = parsed.success && parsed.data.access_key === accessKey ? parsed.data : undefined;
  return <div className="grid gap-2">
    <button type="button" className={ui.button("quiet")} onClick={() => {
      setCopied(false);
      if (!reveal.isIdle) reveal.reset();
      else reveal.mutate({ resource_id: context.resource.id, access_key: accessKey });
    }}>{reveal.isIdle ? "Reveal credentials" : "Hide credentials"}</button>
    {reveal.isPending ? <p role="status">Loading private credentials…</p> : null}
    {reveal.error || reveal.isSuccess && !credential ? <p role="alert">Credentials unavailable. Hide and retry.</p> : null}
    {credential ? <>
      <dl><dt>Access key</dt><dd className={ui.mono}>{credential.access_key}</dd>
        <dt>Secret key</dt><dd className={ui.mono}>{credential.secret_key}</dd>
        <dt>Endpoint</dt><dd>{credential.endpoint}</dd><dt>Bucket</dt><dd>{credential.bucket}</dd></dl>
      <button type="button" className={ui.button("quiet")} onClick={() => {
        // Only mounted reveal state owns the secret; avoid retaining it in a clipboard promise.
        void navigator.clipboard.writeText(credential.secret_key).catch(() => undefined);
        setCopied(true);
      }}>Copy secret key</button>
      {copied ? <p role="status">Copy requested</p> : null}
    </> : null}
  </div>;
}
function Controls({ context, view }: { context: ResourceProps; view: View }) {
  const [confirm, setConfirm] = useState("");
  if (!context.writable || !context.features.includes("manage-buckets")) return null;
  return <section className={ui.card} aria-label="Manage bucket"><div className={ui.cardBody}>
    <h2 className={ui.sectionTitle}>Manage bucket</h2>
    <div className="flex flex-wrap gap-3">
      <Change context={context} view={view} action="usage.inventory" label="Observe next usage page" />
      {view.state === "active" ? <>
        <Change context={context} view={view} action="buckets.deactivate" label="Deactivate and retain data" />
        <Change context={context} view={view} action={view.credentials.length ? "credentials.rotate" : "credentials.create"} label={view.credentials.length ? "Add rotation key" : "Create application key"} />
      </> : null}
      {view.state === "inactive" ? <Change context={context} view={view} action="buckets.reactivate" label="Reactivate without old keys" /> : null}
      {view.state === "needs-reconciliation" ? <Change context={context} view={view} action="buckets.reconcile" label="Reconcile to inactive" /> : null}
    </div>
    <p className="text-sm text-muted">Deactivation retains completed objects and versions and removes lifecycle rules. Incomplete multipart uploads remain subject to provider cleanup; Wasabi automatically removes them after about 31 days.</p>
    <p className="text-sm text-muted">Permanent application keys grant project application access. Rotation overlaps keys until you explicitly revoke the old key. Reveal credentials only when needed, then hide them.</p>
    {view.credentials.map((key) => <div className="flex flex-wrap items-center gap-3" key={key}>
      <code className={ui.mono}>{key}</code>{view.state === "active" ? <CredentialReveal key={`${context.projectID}:${context.resource.id}:${key}:${String(view.revision)}`} context={context} accessKey={key} /> : null}<Change context={context} view={view} action="credentials.revoke" label="Revoke key" extra={{ access_key: key }} />
    </div>)}
    {view.state === "inactive" ? <>
      <p className="text-sm text-muted">Purge removes all objects, versions and incomplete multipart uploads. Final deletion requires a verified empty bucket. Type the resource ID to enable these operations.</p>
      <label className={ui.field}>Confirm {context.resource.id}<input className={ui.input} value={confirm} autoComplete="off" onChange={(event) => { setConfirm(event.target.value); }} /></label>
      {confirm === context.resource.id ? <div className="flex flex-wrap gap-3">
        <Change context={context} view={view} action="buckets.purge" label="Purge next page permanently" destructive extra={{ confirm }} />
        {context.features.includes("manage-buckets") ? <Change context={context} view={view} action="buckets.delete" label="Delete verified empty bucket" destructive extra={{ confirm }} /> : null}
      </div> : null}
    </> : null}
  </div></section>;
}
function Resource(context: ResourceProps) {
  const query = usePluginQuery(context, "buckets.view", { resource_id: context.resource.id }, viewSchema);
  if (query.isPending) return <Loading>Loading bucket…</Loading>;
  if (query.error) return <Failure error={query.error} retry={() => { void query.refetch(); }} />;
  const view = query.data;
  return <div className="grid gap-6">
    <section className={ui.card} aria-label="Bucket status"><div className={ui.cardBody}>
      <h2 className={ui.sectionTitle}>{view.state}</h2>
      <p className="break-all font-mono text-sm">{view.bucket}</p>
      <p className="break-all text-sm">{view.endpoint} · {view.environment}</p>
      <p className="text-sm text-muted">{view.observed ? `Observed ${new Date(view.observed.observed_at * 1000).toLocaleString()}; versioning ${view.observed.versioning ? "enabled" : "suspended or unset"}.` : "Provider observation unavailable. Readiness is unknown."}</p>
      <button className={ui.button("quiet")} type="button" onClick={() => { void query.refetch(); }}>Refresh status</button>
      <p className="text-sm text-muted">Usage is observed and soft. No hard byte quota.</p>
      {view.inventory ? <p className="text-sm">Current: {view.inventory.current_bytes} bytes · Noncurrent: {view.inventory.noncurrent_bytes} bytes · Multipart: {view.inventory.multipart_bytes} bytes. {view.inventory.complete ? "Scan complete" : "Partial scan"}, observed {new Date(view.inventory.observed_at * 1000).toLocaleString()}; concurrent changes may affect totals.</p> : null}
    </div></section>
    <Controls key={`${context.customer}:${context.projectID}:${context.resource.id}`} context={context} view={view} />
    {view.state === "active" && context.actions?.includes("objects.observe") && context.actions.includes("objects.download") ? <BinaryControls key={`${context.projectID}:${context.resource.id}:${String(view.revision)}`} context={context} /> : null}
  </div>;
}
function BinaryControls({ context }: { context: ResourceProps }) {
  const boundary = usePluginScope(context, JSON.stringify([context.writable, context.features, context.actions]));
  return <BinaryAttempt key={boundary.key} context={context} />;
}
function BinaryAttempt({ context }: { context: ResourceProps }) {
  const storage = usePluginStorage(context);
  const [file, setFile] = useState<File | null>(null), [key, setKey] = useState(""), [lookup, setLookup] = useState(""), [hash, setHash] = useState("");
  const validKey = storageKeySchema.safeParse(key).success, validLookup = storageKeySchema.safeParse(lookup).success;
  const phases = { hashing: "Computing file SHA-256…", planning: "Authorizing transfer…", uploading: "Uploading object…", observing: "Inspecting object…", downloading: "Downloading and verifying…" };
  return <section className={ui.card} aria-label="Object transfers"><div className={ui.cardBody}>
    <h2 className={ui.sectionTitle}>Object transfers</h2>
    <p className="text-sm text-muted">Files use authenticated Core transfers. Maximum 512 MiB per file. Uploads create a new object and never overwrite an existing key. Use letters, digits, slash, dot, underscore and hyphen in keys; empty, dot and parent segments are not allowed.</p>
    {context.writable && context.features.includes("manage-buckets") && context.actions?.includes("objects.upload") ? <form aria-label="Upload object" className="mt-4 grid gap-3" onSubmit={event => { event.preventDefault(); if (file) { setLookup(key); setHash(""); void storage.upload(file, key); } }}>
      <label className={ui.field}>File<input type="file" name="storage-file" accept={STORAGE_CONTENT_TYPES.join(",")} disabled={storage.busy} onChange={event => { storage.reset(); const next = event.currentTarget.files?.[0] ?? null; setFile(next); if (!key && next) setKey(next.name.replaceAll(" ", "_")); }} /></label>
      <label className={ui.field}>Upload object key<input className={ui.input} name="storage-upload-key" value={key} maxLength={512} disabled={storage.busy} onChange={event => { storage.reset(); setKey(event.target.value); }} required /></label>
      <p className="text-sm text-muted">Supported types: binary, PDF, ZIP, gzip, plain text and CSV. Filenames use ASCII letters, digits, spaces, dot, underscore and hyphen, up to 160 characters.</p>
      <button className={ui.button("primary")} disabled={storage.busy || storage.status === "uncertain" || !file || !validKey}>Upload object</button>
    </form> : null}
    <form aria-label="Inspect object" className="mt-6 grid gap-3" onSubmit={event => { event.preventDefault(); setHash(""); void storage.observe(lookup); }}>
      <label className={ui.field}>Download object key<input className={ui.input} name="storage-download-key" value={lookup} maxLength={512} disabled={storage.busy} onChange={event => { storage.reset(); setLookup(event.target.value); setHash(""); }} required /></label>
      <button className={ui.button("secondary")} disabled={storage.busy || !validLookup}>Inspect object</button>
    </form>
    {storage.observation ? <div className="mt-4 grid gap-3">
      <p className="break-all text-sm">{storage.observation.filename} · {storage.observation.exact_bytes} bytes · {storage.observation.content_type}</p>
      <p className="text-sm text-muted">{storage.observation.expected_sha256 ? "A full SHA-256 is known and will be verified." : "Full SHA-256 is unavailable. Length will be checked; supply a known SHA-256 for content verification. ETag is not a SHA-256."} The observed version and ETag are fixed for this download.</p>
      <label className={ui.field}>Expected SHA-256 (optional)<input className={ui.input} name="storage-download-sha256" value={hash} maxLength={64} disabled={storage.busy} onChange={event => { storage.reset(true); setHash(event.target.value); }} autoComplete="off" /></label>
      <button type="button" className={ui.button("primary")} disabled={storage.busy || Boolean(hash && !storageHashSchema.safeParse(hash).success)} onClick={() => { void storage.download(hash || undefined); }}>Download observed object</button>
    </div> : null}
    {storage.busy ? <p role="status" className="mt-3 text-sm">{phases[storage.status as keyof typeof phases]}</p> : null}
    {storage.status === "uploaded" ? <p role="status" className="mt-3 break-all text-sm">Upload confirmed. SHA-256: {storage.hash}</p> : null}
    {storage.error ? <p role="alert" className="mt-3 text-sm text-danger">{storage.error}</p> : null}
    {storage.status === "ready" && storage.url ? <div className="mt-3 grid gap-3"><p role="status">Download ready. {storage.hash ? "SHA-256 verified." : "Checksum unknown; exact length verified."}</p><a className={ui.button("primary")} href={storage.url} download={storage.filename}>{storage.hash ? "Save verified file" : "Save length-checked file"}</a></div> : null}
    {storage.busy || storage.url || storage.error ? <button type="button" className={ui.button("quiet")} onClick={() => { storage.reset(); }}>{storage.busy ? "Cancel transfer" : "Hide transfer result"}</button> : null}
  </div></section>;
}
const createdSchema = z.object({ resource: z.object({ id: z.string() }) });
function Create(context: CreateProps) {
  const boundary = usePluginScope(context);
  return <CreateAttempt key={boundary.key} {...context} />;
}
function CreateAttempt(context: CreateProps) {
  const boundary = usePluginScope(context);
  const active = useRef<AbortController | null>(null);
  useEffect(() => () => { active.current?.abort(); active.current = null; }, [boundary.scope]);
  const [submission, setSubmission] = useState<{ action: string; input: Record<string, unknown>; idempotencyKey: string }>();
  const mutation = usePluginSubmission(context, submission?.action ?? context.action, createdSchema);
  return <form className={ui.card} aria-label={context.title} onSubmit={(event) => {
    event.preventDefault();
    const fields = new FormData(event.currentTarget);
    if (!boundary.isCurrent()) return;
    const next = submission ?? { action: context.action, input: { name: fields.get("name"), environment: fields.get("environment"), request_id: crypto.randomUUID() }, idempotencyKey: crypto.randomUUID() };
    setSubmission(next);
    active.current?.abort(); const controller = new AbortController(); active.current = controller;
    mutation.mutate({ ...next, signal: controller.signal }, { onSuccess: (value) => { if (boundary.isCurrent() && active.current === controller && !controller.signal.aborted) context.onCreated(value.resource.id); } });
  }}><div className={ui.cardBody}>
    <h2 className={ui.sectionTitle}>{context.title}</h2>
    <fieldset disabled={Boolean(submission)} className="grid gap-4">
      <label className={ui.field}>Name<input className={ui.input} name="name" required maxLength={63} autoComplete="off" /></label>
      <label className={ui.field}>Environment<input className={ui.input} name="environment" required maxLength={63} autoComplete="off" /></label>
    </fieldset>
    {mutation.error ? <><p role="alert" className="text-sm text-danger">{mutation.error.message} Retry replays the same operation. Check project resources before deliberately starting another create.</p><button type="button" className={ui.button("quiet")} onClick={() => { setSubmission(undefined); mutation.reset(); }}>Discard retry</button></> : null}
    <div className="flex gap-3">
      <button className={ui.button("primary")} disabled={mutation.isPending}>{mutation.isPending ? "Creating…" : mutation.error ? "Retry create bucket" : "Create bucket"}</button>
      <button className={ui.button("quiet")} type="button" disabled={mutation.isPending} onClick={context.onCancel}>Cancel</button>
    </div>
  </div></form>;
}
export default { Resource, Create: { "buckets.create": Create } } satisfies PluginUI;
