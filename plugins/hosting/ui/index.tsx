// Console UI of the hosting plugin. Bundled by the Console; every call goes through the plugin action API.
import { useState, type ReactNode } from "react";
import { z } from "zod";
import { Failure, Loading, ui, usePluginMutation, usePluginQuery, type PluginUI, type ResourceProps } from "@webdock/plugin-ui";

const appSchema = z.object({
  name: z.string(),
  status: z.enum(["deploying", "running", "stopped", "failed", "missing"]),
  image: z.string(),
  port: z.number(),
  memoryMb: z.number(),
  url: z.string().nullable(),
  env: z.array(z.string()),
  error: z.string().nullable(),
  createdAt: z.string(),
  host: z.string(),
  domains: z.array(z.string()),
});
type App = z.infer<typeof appSchema>;
const logsSchema = z.object({ lines: z.array(z.string()) });
const statusText = { deploying: "Deploying", running: "Running", stopped: "Stopped", failed: "Failed", missing: "Missing" } as const;
const statusTone = { deploying: "warning", running: "success", stopped: "neutral", failed: "danger", missing: "danger" } as const;
const memory = [128, 256, 512, 1024, 2048];

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className={ui.card} aria-label={title}>
      <div className={ui.cardBody}>
        <h2 className={ui.sectionTitle}>{title}</h2>
        {children}
      </div>
    </section>
  );
}
function Problem({ error }: { error: Error | null }) {
  return error ? <p role="alert" className="text-sm text-danger">{error.message}</p> : null;
}

function Power({ context, app }: { context: ResourceProps; app: App }) {
  const power = usePluginMutation(context, "apps.power");
  const busy = power.isPending;
  const run = (action: string) => { power.mutate({ resource_id: context.resource.id, action }); };
  return (
    <div className="flex flex-wrap gap-2">
      {app.status === "stopped" ? (
        <button type="button" className={ui.button()} disabled={busy} onClick={() => { run("start"); }}>Start</button>
      ) : (
        <>
          <button type="button" className={ui.button()} disabled={busy || app.status !== "running"} onClick={() => { run("restart"); }}>Restart</button>
          <button type="button" className={ui.button()} disabled={busy} onClick={() => { run("stop"); }}>Stop</button>
        </>
      )}
      <Problem error={power.error} />
    </div>
  );
}

function Deploy({ context, app }: { context: ResourceProps; app: App }) {
  const deploy = usePluginMutation(context, "apps.deploy");
  return (
    <Section title="Deploy">
      <form
        className="grid gap-4 sm:grid-cols-[1fr_8rem_auto] sm:items-end"
        onSubmit={(event) => {
          event.preventDefault();
          const data = Object.fromEntries(new FormData(event.currentTarget)) as Record<string, string>;
          deploy.mutate({ resource_id: context.resource.id, image: data.image, port: Number(data.port) });
        }}
      >
        <label className={ui.field}>
          Image
          <input className={ui.input} name="image" required maxLength={512} defaultValue={app.image} />
        </label>
        <label className={ui.field}>
          Port
          <input className={ui.input} name="port" type="number" required min={1} max={65535} defaultValue={app.port} />
        </label>
        <button className={ui.button("primary")} disabled={deploy.isPending}>Deploy</button>
      </form>
      <Problem error={deploy.error} />
    </Section>
  );
}

function Environment({ context, app }: { context: ResourceProps; app: App }) {
  const save = usePluginMutation(context, "apps.env");
  const [editing, setEditing] = useState(false);
  return (
    <Section title="Environment variables">
      {app.env.length ? (
        <ul className="flex flex-wrap gap-2" aria-label="Variable names">
          {app.env.map((key) => <li key={key}><code className={ui.cx(ui.mono, "rounded-md border border-outline px-2 py-1")}>{key}</code></li>)}
        </ul>
      ) : <p className="text-sm text-muted">No variables set.</p>}
      <p className="text-[13px] text-muted">Values are stored encrypted and never shown again. Saving replaces all variables and redeploys the app.</p>
      {editing ? (
        <form
          className="grid gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            save.mutate({ resource_id: context.resource.id, env: (Object.fromEntries(new FormData(event.currentTarget)) as Record<string, string>).env }, { onSuccess: () => { setEditing(false); } });
          }}
        >
          <label className={ui.field}>
            Variables, one KEY=value per line
            <textarea className={ui.cx(ui.input, "min-h-36 font-mono text-xs")} name="env" maxLength={30000} spellCheck={false} placeholder={app.env.map((key) => `${key}=`).join("\n") || "DATABASE_URL=postgresql://…"} />
          </label>
          <div className="flex gap-2">
            <button className={ui.button("primary")} disabled={save.isPending}>{save.isPending ? "Saving…" : "Save and redeploy"}</button>
            <button type="button" className={ui.button("quiet")} onClick={() => { setEditing(false); }}>Cancel</button>
          </div>
          <Problem error={save.error} />
        </form>
      ) : (
        <div><button type="button" className={ui.button()} onClick={() => { setEditing(true); }}>Replace variables</button></div>
      )}
    </Section>
  );
}

function Domains({ context, app }: { context: ResourceProps; app: App }) {
  const save = usePluginMutation(context, "apps.domains");
  const [editing, setEditing] = useState(false);
  return (
    <Section title="Domains">
      <ul className="grid gap-1 text-sm" aria-label="Domains">
        <li><code className={ui.mono}>{app.host}</code> <span className="text-muted">(always)</span></li>
        {app.domains.map((domain) => <li key={domain}><code className={ui.mono}>{domain}</code></li>)}
      </ul>
      <p className="text-[13px] text-muted">
        Verify a domain under Domains in your workspace first, then point it at <code className={ui.mono}>{app.host}</code> with a CNAME record. Each domain gets its own certificate.
      </p>
      {editing ? (
        <form
          className="grid gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            save.mutate({ resource_id: context.resource.id, domains: (Object.fromEntries(new FormData(event.currentTarget)) as Record<string, string>).domains }, { onSuccess: () => { setEditing(false); } });
          }}
        >
          <label className={ui.field}>
            Own domains, one per line
            <textarea className={ui.cx(ui.input, "min-h-24 font-mono text-xs")} name="domains" maxLength={2600} spellCheck={false} defaultValue={app.domains.join("\n")} placeholder="shop.example.com" />
          </label>
          <div className="flex gap-2">
            <button className={ui.button("primary")} disabled={save.isPending}>{save.isPending ? "Saving…" : "Save and redeploy"}</button>
            <button type="button" className={ui.button("quiet")} onClick={() => { setEditing(false); }}>Cancel</button>
          </div>
          <Problem error={save.error} />
        </form>
      ) : (
        <div><button type="button" className={ui.button()} onClick={() => { setEditing(true); }}>Change domains</button></div>
      )}
    </Section>
  );
}

function Memory({ context, app }: { context: ResourceProps; app: App }) {
  const resize = usePluginMutation(context, "apps.resize");
  return (
    <label className={ui.field}>
      Memory
      <select
        className={ui.cx(ui.input, "w-40")}
        value={app.memoryMb}
        disabled={resize.isPending}
        onChange={(event) => { resize.mutate({ resource_id: context.resource.id, memory_mb: Number(event.target.value) }); }}
      >
        {memory.map((mb) => <option key={mb} value={mb}>{mb >= 1024 ? `${String(mb / 1024)} GB` : `${String(mb)} MB`}</option>)}
      </select>
      <Problem error={resize.error} />
    </label>
  );
}

function Logs({ context }: { context: ResourceProps }) {
  const logs = usePluginQuery(context, "apps.logs", { resource_id: context.resource.id }, logsSchema);
  return (
    <section className={ui.card} aria-label="Logs">
      <div className="flex items-center justify-between px-6 py-4 max-sm:px-[18px]">
        <h2 className={ui.sectionTitle}>Logs</h2>
        <button type="button" className={ui.button("quiet")} disabled={logs.isFetching} onClick={() => void logs.refetch()}>Refresh</button>
      </div>
      {logs.error ? <div className="px-6 pb-5"><Problem error={logs.error} /></div> : (
        <pre ref={(element) => { if (element) element.scrollTop = element.scrollHeight; }} className="max-h-96 overflow-auto border-t border-outline bg-subtle px-6 py-4 font-mono text-xs leading-5 max-sm:px-[18px]">
          {logs.data?.lines.length ? logs.data.lines.join("\n") : logs.isPending ? "Loading…" : "No output yet."}
        </pre>
      )}
    </section>
  );
}

function Remove({ context }: { context: ResourceProps }) {
  const remove = usePluginMutation(context, "apps.delete", { leaves: true });
  const [typed, setTyped] = useState("");
  if (!context.features.includes("create-apps")) return null;
  return (
    <Section title="Delete app">
      <p className="text-[13px] text-muted">The app stops and is removed. Type {context.resource.name} to confirm.</p>
      <form className="flex flex-wrap gap-2" onSubmit={(event) => { event.preventDefault(); remove.mutate({ resource_id: context.resource.id }, { onSuccess: context.onDeleted }); }}>
        <input className={ui.cx(ui.input, "max-w-64")} aria-label={`Type ${context.resource.name} to confirm`} value={typed} autoComplete="off" onChange={(event) => { setTyped(event.target.value); }} />
        <button className={ui.button("danger")} disabled={typed !== context.resource.name || remove.isPending}>{remove.isPending ? "Deleting…" : "Delete app"}</button>
      </form>
      <Problem error={remove.error} />
    </Section>
  );
}

function Resource(context: ResourceProps) {
  const query = usePluginQuery(context, "apps.view", { resource_id: context.resource.id }, appSchema, {
    refetchInterval: (data) => (data?.status === "deploying" ? 2000 : false),
  });
  if (query.isPending) return <Loading>Loading app…</Loading>;
  if (query.error) return <Failure error={query.error} retry={() => void query.refetch()} />;
  const app = query.data;
  return (
    <div className="grid gap-6">
      <section className={ui.card} aria-label="Status">
        <div className={ui.cardBody}>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex flex-wrap items-center gap-3">
              <span className={ui.badge(statusTone[app.status])}>{statusText[app.status]}</span>
              {app.url ? <a className="text-sm font-medium" href={app.url} target="_blank" rel="noreferrer">{app.url}</a> : null}
            </div>
            {context.writable ? <Power context={context} app={app} /> : null}
          </div>
          {app.error ? <p role="alert" className="text-sm text-danger">{app.error}</p> : null}
          <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm">
            <dt className="text-muted">Image</dt>
            <dd><code className={ui.mono}>{app.image}</code></dd>
            <dt className="text-muted">Port</dt>
            <dd>{app.port}</dd>
            {!context.writable || !context.features.includes("resize-apps") ? (
              <>
                <dt className="text-muted">Memory</dt>
                <dd>{app.memoryMb} MB</dd>
              </>
            ) : null}
          </dl>
          {context.writable && context.features.includes("resize-apps") ? <Memory context={context} app={app} /> : null}
        </div>
      </section>
      {context.writable ? <Deploy context={context} app={app} /> : null}
      {context.writable ? <Domains context={context} app={app} /> : null}
      {context.writable ? <Environment context={context} app={app} /> : null}
      {context.writable ? <Logs context={context} /> : null}
      {context.writable ? <Remove context={context} /> : null}
    </div>
  );
}

export default { Resource } satisfies PluginUI;
