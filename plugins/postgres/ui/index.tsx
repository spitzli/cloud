// Console UI of the PostgreSQL plugin. Bundled by the Console; every call goes through the plugin action API.
import { useState } from "react";
import { z } from "zod";
import { Failure, Loading, ui, usePluginMutation, usePluginQuery, usePluginReveal, type PluginUI, type ResourceProps } from "@webdock/plugin-ui";

const viewSchema = z.object({
  offering: z.enum(["shared", "dedicated"]),
  state: z.enum(["creating", "ready", "failed"]),
  host: z.string().nullable(),
  port: z.number().nullable(),
  database: z.string(),
  user: z.string(),
  version: z.string().nullable(),
  server: z.string().nullable(),
  error: z.string().nullable(),
  createdAt: z.string(),
  sslMode: z.string(),
});
type View = z.infer<typeof viewSchema>;
const secretSchema = z.object({ value: z.string(), uri: z.string() });
const stateText = { creating: "Creating", ready: "Ready", failed: "Failed" } as const;
const stateTone = { creating: "warning", ready: "success", failed: "danger" } as const;

function Copy({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className={ui.button("quiet")}
      onClick={() => {
        void navigator.clipboard.writeText(value).then(() => {
          setCopied(true);
          setTimeout(() => { setCopied(false); }, 1500);
        });
      }}
    >
      {copied ? "Copied" : "Copy"}
    </button>
  );
}
function Detail({ label, value }: { label: string; value: string }) {
  return (
    <div className={ui.cardRow}>
      <span className="w-28 shrink-0 text-muted">{label}</span>
      <code className={ui.cx(ui.mono, "min-w-0 flex-1 break-all text-[13px]")}>{value}</code>
      <Copy value={value} />
    </div>
  );
}

function Credentials({ context, resource }: { context: ResourceProps; resource: string }) {
  const reveal = usePluginReveal(context, "databases.credentials");
  const secret = reveal.data ? secretSchema.safeParse(reveal.data) : undefined;
  return (
    <div className={ui.cardRow}>
      <span className="w-28 shrink-0 text-muted">Password</span>
      {secret?.success ? (
        <>
          <code className={ui.cx(ui.mono, "min-w-0 flex-1 break-all text-[13px]")}>{secret.data.value}</code>
          <Copy value={secret.data.value} />
          <button type="button" className={ui.button("quiet")} onClick={() => { reveal.reset(); }}>Hide</button>
          <div className="basis-full">
            <p className="mb-1 text-[13px] text-muted">Connection URI</p>
            <div className="flex items-center gap-2">
              <code className={ui.cx(ui.mono, "min-w-0 flex-1 break-all rounded-md border border-outline bg-subtle px-3 py-2")}>{secret.data.uri}</code>
              <Copy value={secret.data.uri} />
            </div>
          </div>
        </>
      ) : (
        <>
          <span className="flex-1 text-muted">Hidden</span>
          <button type="button" className={ui.button()} disabled={reveal.isPending} onClick={() => { reveal.mutate({ resource_id: resource }); }}>
            {reveal.isPending ? "Loading…" : "Show password"}
          </button>
        </>
      )}
      {reveal.error || secret?.success === false ? <p role="alert" className="basis-full text-danger">Could not load the password. Try again.</p> : null}
    </div>
  );
}

function Actions({ context, view }: { context: ResourceProps; view: View }) {
  const resource = { resource_id: context.resource.id };
  const rotate = usePluginMutation(context, "databases.rotate");
  const remove = usePluginMutation(context, view.offering === "shared" ? "databases.delete" : "instances.delete", { leaves: true });
  const [confirming, setConfirming] = useState(false);
  const [typed, setTyped] = useState("");
  const feature = view.offering === "shared" ? "create-shared" : "create-dedicated";
  const canDelete = context.writable && context.features.includes(feature) && view.state !== "creating";
  if (!context.writable) return null;
  return (
    <section className={ui.card} aria-label="Manage">
      <div className={ui.cardBody}>
        <h2 className={ui.sectionTitle}>Manage</h2>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <p className="text-sm font-medium">Rotate password</p>
            <p className="text-[13px] text-muted">Applications using the current password lose access until you update them.</p>
          </div>
          <button type="button" className={ui.button()} disabled={rotate.isPending || view.state !== "ready"} onClick={() => { rotate.mutate(resource); }}>
            {rotate.isPending ? "Rotating…" : rotate.isSuccess ? "Rotated" : "Rotate password"}
          </button>
        </div>
        {rotate.error ? <p role="alert" className="text-sm text-danger">{rotate.error.message}</p> : null}
        {canDelete ? (
          <div className="grid gap-3 border-t border-outline pt-4">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <p className="text-sm font-medium">Delete {view.offering === "shared" ? "database" : "instance"}</p>
                <p className="text-[13px] text-muted">All data is removed. This cannot be undone.</p>
              </div>
              {!confirming ? <button type="button" className={ui.button("danger")} onClick={() => { setConfirming(true); }}>Delete</button> : null}
            </div>
            {confirming ? (
              <form
                className="grid gap-3"
                onSubmit={(event) => {
                  event.preventDefault();
                  remove.mutate(resource, { onSuccess: context.onDeleted });
                }}
              >
                <label className={ui.field}>
                  Type {context.resource.name} to confirm
                  <input className={ui.input} value={typed} autoComplete="off" onChange={(event) => { setTyped(event.target.value); }} />
                </label>
                <div className="flex gap-2">
                  <button type="submit" className={ui.button("danger")} disabled={typed !== context.resource.name || remove.isPending}>
                    {remove.isPending ? "Deleting…" : "Delete permanently"}
                  </button>
                  <button type="button" className={ui.button("quiet")} onClick={() => { setConfirming(false); setTyped(""); }}>Cancel</button>
                </div>
                {remove.error ? <p role="alert" className="text-sm text-danger">{remove.error.message}</p> : null}
              </form>
            ) : null}
          </div>
        ) : null}
      </div>
    </section>
  );
}

function Resource(context: ResourceProps) {
  const view = usePluginQuery(context, "databases.view", { resource_id: context.resource.id }, viewSchema, {
    refetchInterval: (data) => (data?.state === "creating" ? 2000 : false),
  });
  if (view.isPending) return <Loading>Loading database…</Loading>;
  if (view.error) return <Failure error={view.error} retry={() => void view.refetch()} />;
  const data = view.data;
  const kind = data.offering === "shared" ? `Shared database${data.server ? ` on ${data.server}` : ""}` : `Dedicated PostgreSQL ${data.version ?? ""}`;
  return (
    <div className="grid gap-6">
      <section className={ui.card} aria-label="Status">
        <div className={ui.cx(ui.cardBody, "grid-cols-[auto_1fr] items-center gap-x-4")}>
          <span className={ui.badge(stateTone[data.state])}>{stateText[data.state]}</span>
          <p className="text-sm text-muted">{kind}</p>
          {data.state === "creating" ? <p className="col-span-2 text-[13px] text-muted">Starting PostgreSQL. This page updates by itself.</p> : null}
          {data.state === "failed" ? <p role="alert" className="col-span-2 text-sm text-danger">{data.error ?? "Something went wrong."}</p> : null}
        </div>
      </section>
      {data.state === "ready" && data.host && data.port ? (
        <section className={ui.card} aria-label="Connection">
          <h2 className={ui.cx(ui.sectionTitle, "px-6 py-4 max-sm:px-[18px]")}>Connection</h2>
          <Detail label="Host" value={data.host} />
          <Detail label="Port" value={String(data.port)} />
          <Detail label="Database" value={data.database} />
          <Detail label="User" value={data.user} />
          {data.sslMode === "require" ? <Detail label="SSL mode" value="require" /> : null}
          {context.writable ? <Credentials context={context} resource={context.resource.id} /> : null}
        </section>
      ) : null}
      <Actions context={context} view={data} />
    </div>
  );
}

export default { Resource } satisfies PluginUI;
