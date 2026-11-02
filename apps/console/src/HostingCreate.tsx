import { useState } from "react";
import { FieldError, useLiveValidation } from "./live-validation";
import { Link, useBlocker } from "@tanstack/react-router";
import { checkArguments, createCommand, fieldRules } from "./hosting-contracts";
import { useHostingAction } from "./hosting-client";
import { HostingReceipt } from "./HostingOperation";
import { ActionFailure } from "./HostingActionState";
import { HostingEnvironment } from "./HostingEnvironment";
import * as ui from "./ui";
export function HostingCreate({ projectID, subscriptionRevision, publicSitePolicy }: { projectID: string; subscriptionRevision: number; publicSitePolicy?:{domain:string|null;enabled:boolean}|null|undefined }) {
  const [open, setOpen] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [validation, setValidation] = useState<string>();
  const live = useLiveValidation();
  const { mutation, retry, uncertain } = useHostingAction(() => { setDirty(false); });
  useBlocker({
    shouldBlockFn: () => (dirty || uncertain) && !window.confirm("Leave this view? The new application has not been requested, or its request outcome is unknown."),
    enableBeforeUnload: dirty || uncertain,
  });
  const locked = mutation.isPending || uncertain || Boolean(mutation.data);
  if (!open)
    return <button className={ui.cx(`${ui.button("primary")} mb-6`)} onClick={() => { setOpen(true); }}>New application</button>;
  return <section className={ui.cx(`${ui.card} mb-6`)} aria-label="New application">
    <form {...live.form} onChange={() => { setDirty(true); }} onSubmit={(event) => {
      event.preventDefault();
      try {
        const command = createCommand(projectID, subscriptionRevision, new FormData(event.currentTarget), crypto.randomUUID());
        setValidation(undefined);
        mutation.mutate(command);
      } catch {
        setValidation("Check the name, image digest, arguments, resources and environment variables. Persistent storage supports one replica.");
      }
    }}>
      <div className={ui.cx(`${ui.cardBody} gap-5`)}>
      <h2 className={ui.sectionTitle}>New application</h2>
      <fieldset disabled={locked} className="grid min-w-0 gap-4 border-0 p-0">
        <legend className="px-0 pb-2 text-sm font-semibold">Deployment configuration</legend>
        <label className={ui.field}>Application name<input className={ui.input} name="name" required maxLength={63} autoComplete="off" /></label><FieldError message={live.errors.name} />
        <label className={ui.field}>Image digest<input className={ui.input} name="image" pattern={fieldRules.image.pattern} title={fieldRules.image.title} aria-label="Image digest" required maxLength={512} placeholder="registry.example/app@sha256:…" autoComplete="off" spellCheck={false} />
          <span className={ui.hint}>An immutable image reference pinned by digest.</span></label><FieldError message={live.errors.image} />
        <label className={ui.field}>Startup arguments<textarea className={ui.cx(`${ui.input} font-mono text-xs`)} name="args" onInput={(event) => { checkArguments(event.currentTarget); }} aria-label="Startup arguments" rows={3} required defaultValue="[]" />
          <span className={ui.hint}>A JSON list of arguments. Use [] for the image default.</span></label><FieldError message={live.errors.args} />
        {publicSitePolicy?.enabled && publicSitePolicy.domain ? <label className="flex items-start gap-3 text-sm"><input type="checkbox" name="publicAddress" defaultChecked /><span>Public website or app<span className="mt-1 block text-xs text-muted">Assign an HTTPS address under {publicSitePolicy.domain}. Turn this off for a private service. You can add your own domain separately.</span></span></label>:null}
        <label className={ui.field}>Application port<input className={ui.input} name="port" type="number" min={1024} max={65535} required defaultValue={8080} /></label><FieldError message={live.errors.port} />
        <label className={ui.field}>Health path<input className={ui.input} name="healthPath" pattern={fieldRules.healthPath.pattern} title={fieldRules.healthPath.title} required maxLength={160} defaultValue="/" /></label><FieldError message={live.errors.healthPath} />
        <label className={ui.field}>CPU<input className={ui.input} name="cpuMillicores" pattern={fieldRules.cpuMillicores.pattern} title={fieldRules.cpuMillicores.title} aria-label="CPU" required defaultValue="0.1 cores" /></label><FieldError message={live.errors.cpuMillicores} />
        <label className={ui.field}>Memory<input className={ui.input} name="memoryBytes" pattern={fieldRules.bytes.pattern} title={fieldRules.bytes.title} aria-label="Memory" required defaultValue="128 MB" /></label><FieldError message={live.errors.memoryBytes} />
        <label className={ui.field}>Temporary storage<input className={ui.input} name="ephemeralBytes" pattern={fieldRules.bytes.pattern} title={fieldRules.bytes.title} required defaultValue="64 MB" /></label><FieldError message={live.errors.ephemeralBytes} />
        <label className={ui.field}>Replicas<input className={ui.input} name="replicas" type="number" min={0} max={20} step={1} required defaultValue={1} /></label><FieldError message={live.errors.replicas} />
        <label className={ui.field}>Persistent storage<input className={ui.input} name="volumeBytes" pattern={fieldRules.bytes.pattern} title={fieldRules.bytes.title} aria-label="Persistent storage" required defaultValue="0 MB" />
          <span className={ui.hint}>0 MB for none; otherwise at least 64 MB and one replica.</span></label><FieldError message={live.errors.volumeBytes} />
      </fieldset>
      <HostingEnvironment names={[]} disabled={locked} changed={() => { setDirty(true); }} />
      {validation ? <p className="text-sm text-danger" role="alert">{validation}</p> : null}
      <ActionFailure error={mutation.error} uncertain={uncertain} retry={retry} pending={mutation.isPending} />
      {mutation.data ? <>
        <HostingReceipt receipt={mutation.data} />
        <Link className={ui.cx(`${ui.button()} justify-self-start`)} to="/hosting/apps/$appID" params={{ appID: mutation.data.appID }}>Open new application</Link>
      </> : null}
      </div>
      <div className={ui.cx(`${ui.cardFooter} justify-end`)}>
        <button type="button" className={ui.button()} disabled={mutation.isPending || uncertain} onClick={() => {
          if (dirty && !window.confirm("Discard this new application?")) return;
          setDirty(false); setOpen(false); mutation.reset();
        }}>Close</button>
        <button className={ui.button("primary")} disabled={locked}>{mutation.isPending ? "Submitting…" : "Request application"}</button>
      </div>
    </form>
  </section>;
}
