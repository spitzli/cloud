// Console UI of the mail plugin. Bundled by the Console; every call goes through the plugin action API.
import { useState } from "react";
import { SendForm, Outbox } from "./Send";
import { z } from "zod";
import { Failure, Loading, ui, usePluginMutation, usePluginQuery, usePluginReveal, type CreateProps, type PluginUI, type ResourceProps } from "@webdock/plugin-ui";

const domainSchema = z.object({
  domain: z.string(),
  verified: z.boolean(),
  verificationSkipped: z.boolean(),
  transport: z.enum(["platform", "turbosmtp"]),
  records: z.array(z.object({ type: z.string(), name: z.string(), value: z.string(), purpose: z.string() })),
  mailboxes: z.array(z.string()),
  incoming: z.object({ state: z.enum(["disabled", "provisioning", "ready", "failed", "suspending", "suspended", "migration-required"]), desiredEnabled: z.boolean() }),
  recent: z.array(z.object({ from: z.string(), to: z.string(), subject: z.string(), transport: z.string(), status: z.enum(["accepted", "failed"]), at: z.string() })),
});
type Domain = z.infer<typeof domainSchema>;
const mailboxSchema = z.object({
  address: z.string(),
  name: z.string(),
  state: z.enum(["creating", "ready", "failed", "suspending", "suspended", "migration-required"]),
  error: z.string().nullable(),
  host: z.string().nullable(),
  imapPort: z.number(),
  smtpPort: z.number(),
  username: z.string(),
  quotaMb: z.number(),
  tls: z.string(),
});
const createdSchema = z.object({ resource: z.object({ id: z.string() }) });
const relaySchema = z.object({ provider: z.string(), region: z.string(), keyHint: z.string(), connectedAt: z.string() });
const verifySchema = z.object({ verified: z.boolean() });

function Copy({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className={ui.cx(ui.button("quiet"), "relative z-10")}
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

function Remove({ context, action, noun, feature }: { context: ResourceProps; action: string; noun: string; feature: string }) {
  const remove = usePluginMutation(context, action, { leaves: true });
  const [confirming, setConfirming] = useState(false);
  if (!context.writable || !context.features.includes(feature)) return null;
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 border-t border-outline pt-4 first:border-t-0 first:pt-0">
      <div>
        <p className="text-sm font-medium">Remove {noun}</p>
        <p className="text-[13px] text-muted">{({ domain: "Mail can no longer be sent from this domain.", mailbox: "The mailbox and all its mail are deleted.", account: "Mail goes through the platform again." } as Record<string, string>)[noun]}</p>
      </div>
      {confirming ? (
        <div className="flex gap-2">
          <button type="button" className={ui.button("danger")} disabled={remove.isPending} onClick={() => { remove.mutate({ resource_id: context.resource.id }, { onSuccess: context.onDeleted }); }}>
            {remove.isPending ? "Removing…" : `Remove ${noun}`}
          </button>
          <button type="button" className={ui.button("quiet")} onClick={() => { setConfirming(false); }}>Cancel</button>
        </div>
      ) : (
        <button type="button" className={ui.button("danger")} onClick={() => { setConfirming(true); }}>Remove</button>
      )}
      {remove.error ? <p role="alert" className="basis-full text-sm text-danger">{remove.error.message}</p> : null}
    </div>
  );
}

function IncomingMail({ context, domain, refresh }: { context: ResourceProps; domain: Domain; refresh: () => void }) {
  const activate = usePluginMutation(context, "incoming.activate");
  const deactivate = usePluginMutation(context, "incoming.deactivate");
  const [confirmed, setConfirmed] = useState(false);
  const { state, desiredEnabled } = domain.incoming;
  const pending = activate.isPending || deactivate.isPending;
  const change = () => {
    (desiredEnabled ? deactivate : activate).mutate({ resource_id: context.resource.id }, { onSuccess: () => { setConfirmed(false); refresh(); } });
  };
  return (
    <section className={ui.card} aria-label="Customer incoming mail">
      <div className={ui.cardBody}>
        <h2 className={ui.sectionTitle}>Incoming mail</h2>
        <p className="text-sm text-muted">All projects and domains of this customer share one isolated mail server. Suspending it retains mailboxes and stored mail.</p>
        <p className="text-sm">{state === "migration-required" ? "Operator migration required: retained project mail servers must be migrated before incoming mail can change." : state === "disabled" ? "Not activated. Outgoing mail does not allocate a mail server." : `Status: ${state}`}</p>
        {state !== "disabled" && state !== "migration-required" ? <p className="text-sm text-muted">API sending through incoming mail is pending relay integration. Public reception is not enabled.</p> : null}
        {context.writable && context.features.includes("manage-incoming") && state !== "migration-required" ? (
          <div className="grid gap-3">
            {desiredEnabled ? <label className="text-sm"><input type="checkbox" checked={confirmed} onChange={(event) => { setConfirmed(event.target.checked); }} /> Suspend incoming mail for every project of this customer</label> : null}
            <button type="button" className={ui.button()} disabled={pending || (desiredEnabled && !confirmed)} onClick={() => { change(); }}>
              {pending ? "Applying…" : desiredEnabled ? "Suspend incoming mail" : "Activate incoming mail"}
            </button>
          </div>
        ) : null}
        {activate.error || deactivate.error ? <p role="alert" className="text-sm text-danger">{(activate.error ?? deactivate.error)?.message}</p> : null}
      </div>
    </section>
  );
}

function DomainPage(context: ResourceProps) {
  const query = usePluginQuery(context, "domains.view", { resource_id: context.resource.id }, domainSchema, { refetchInterval: (data) => data && ["provisioning", "suspending"].includes(data.incoming.state) ? 2000 : false });
  const verify = usePluginMutation(context, "domains.verify");
  if (!query.data) return query.error ? <Failure error={query.error} retry={() => void query.refetch()} /> : <Loading>Loading domain…</Loading>;
  const domain = query.data;
  const checked = verify.data ? verifySchema.safeParse(verify.data) : undefined;
  return (
    <div className="grid gap-6">
      {query.error ? <Failure error={query.error} retry={() => void query.refetch()} /> : null}
      <section className={ui.card} aria-label="Status">
        <div className={ui.cardBody}>
          <div className="flex flex-wrap items-center gap-3">
            <span className={ui.badge(domain.verified ? "success" : "warning")}>{domain.verified ? "Verified" : "Not verified"}</span>
            <span className="text-sm text-muted">
              {domain.incoming.state !== "disabled" ? "Incoming mail uses the customer’s mail server" : domain.transport === "turbosmtp" ? "Sends through your own turboSMTP account" : "Sends through the platform"}
            </span>
          </div>
          {domain.verificationSkipped ? <p className={ui.notice}>Local development: DNS ownership is not checked here.</p> : null}
          {!domain.verified ? (
            <div className="flex flex-wrap items-center gap-3">
              <p className="text-sm">Add the ownership record below at your DNS provider, then check it.</p>
              {context.writable ? (
                <button type="button" className={ui.button()} disabled={verify.isPending} onClick={() => { verify.mutate({ resource_id: context.resource.id }); }}>
                  {verify.isPending ? "Checking…" : "Check DNS"}
                </button>
              ) : null}
              {checked?.success && !checked.data.verified ? <p role="alert" className="text-sm text-danger">The record was not found yet. DNS changes can take a while.</p> : null}
            </div>
          ) : null}
        </div>
      </section>
      <IncomingMail context={context} domain={domain} refresh={() => void query.refetch()} />
      <section className={ui.card} aria-label="DNS records">
        <h2 className={ui.cx(ui.sectionTitle, "px-6 py-4 max-sm:px-[18px]")}>DNS records</h2>
        {domain.records.map((record) => (
          <div key={record.name + record.value} className={ui.cardRow}>
            <div className="grid min-w-0 flex-1 gap-1">
              <span className="text-[13px] text-muted">{record.purpose}</span>
              <span className="text-sm"><span className="font-medium">{record.type}</span> <code className={ui.mono}>{record.name}</code></span>
              <code className={ui.cx(ui.mono, "break-all")}>{record.value}</code>
            </div>
            <Copy value={record.value} />
          </div>
        ))}
      </section>
      {domain.mailboxes.length ? (
        <section className={ui.card} aria-label="Mailboxes on this domain">
          <h2 className={ui.cx(ui.sectionTitle, "px-6 py-4 max-sm:px-[18px]")}>Mailboxes</h2>
          {domain.mailboxes.map((address) => <div key={address} className={ui.cardRow}><span>{address}</span></div>)}
        </section>
      ) : null}
      {context.writable && domain.incoming.state === "disabled" ? <SendForm context={context} domain={domain} /> : null}
      <Outbox context={context} />
      <section className={ui.card} aria-label="Recent messages">
        <h2 className={ui.cx(ui.sectionTitle, "px-6 py-4 max-sm:px-[18px]")}>Recent messages</h2>
        {domain.recent.length === 0 ? <p className="px-6 pb-5 text-sm text-muted max-sm:px-[18px]">Nothing sent yet.</p> : domain.recent.map((message) => (
          <div key={message.at + message.to} className={ui.cardRow}>
            <div className="grid min-w-0 flex-1 gap-0.5">
              <span className="truncate font-medium">{message.subject}</span>
              <span className="truncate text-[13px] text-muted">{message.from} to {message.to}</span>
            </div>
            <span className={ui.badge(message.status === "accepted" ? "success" : "danger")}>{message.status === "accepted" ? "Sent" : "Failed"}</span>
          </div>
        ))}
      </section>
      <div className={ui.cx(ui.card, ui.cardBody)}>
        <Remove context={context} action="domains.delete" noun="domain" feature="add-domains" />
      </div>
    </div>
  );
}

function RelayPage(context: ResourceProps) {
  const query = usePluginQuery(context, "relay.view", { resource_id: context.resource.id }, relaySchema);
  if (query.isPending) return <Loading>Loading account…</Loading>;
  if (query.error) return <Failure error={query.error} retry={() => void query.refetch()} />;
  const relay = query.data;
  return (
    <section className={ui.card} aria-label="Account">
      <div className={ui.cardBody}>
        <p className="text-sm">All mail of this project goes through this {relay.provider} account ({relay.region} region).</p>
        <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm">
          <dt className="text-muted">Consumer key</dt>
          <dd><code className={ui.mono}>{relay.keyHint}</code></dd>
          <dt className="text-muted">Secret</dt>
          <dd className="text-muted">Stored encrypted, never shown again</dd>
          <dt className="text-muted">Connected</dt>
          <dd>{relay.connectedAt.slice(0, 16)}</dd>
        </dl>
        <Remove context={context} action="relay.delete" noun="account" feature="byok-turbosmtp" />
      </div>
    </section>
  );
}


function MailboxPassword({ context }: { context: ResourceProps }) {
  const reveal = usePluginReveal(context, "mailboxes.credentials");
  const rotate = usePluginMutation(context, "mailboxes.rotate");
  const secret = reveal.data ? z.object({ value: z.string() }).safeParse(reveal.data) : undefined;
  return (
    <div className={ui.cardRow}>
      <span className="w-36 shrink-0 text-muted">Password</span>
      {secret?.success ? (
        <>
          <code className={ui.cx(ui.mono, "min-w-0 flex-1 break-all")}>{secret.data.value}</code>
          <Copy value={secret.data.value} />
          <button type="button" className={ui.button("quiet")} onClick={() => { reveal.reset(); }}>Hide</button>
        </>
      ) : (
        <>
          <span className="flex-1 text-muted">Hidden</span>
          <button type="button" className={ui.button()} disabled={reveal.isPending} onClick={() => { reveal.mutate({ resource_id: context.resource.id }); }}>
            {reveal.isPending ? "Loading…" : "Show password"}
          </button>
        </>
      )}
      <button type="button" className={ui.button()} disabled={rotate.isPending} onClick={() => { reveal.reset(); rotate.mutate({ resource_id: context.resource.id }); }}>
        {rotate.isPending ? "Rotating…" : rotate.isSuccess ? "Rotated" : "Rotate"}
      </button>
      {reveal.error ?? rotate.error ? <p role="alert" className="basis-full text-sm text-danger">{(reveal.error ?? rotate.error)?.message}</p> : null}
    </div>
  );
}

function MailboxPage(context: ResourceProps) {
  const query = usePluginQuery(context, "mailboxes.view", { resource_id: context.resource.id }, mailboxSchema, {
    refetchInterval: (data) => (data && ["creating", "suspending"].includes(data.state) ? 2000 : false),
  });
  if (query.isPending) return <Loading>Loading mailbox…</Loading>;
  if (query.error) return <Failure error={query.error} retry={() => void query.refetch()} />;
  const mailbox = query.data;
  const row = (label: string, value: string) => (
    <div className={ui.cardRow}>
      <span className="w-36 shrink-0 text-muted">{label}</span>
      <code className={ui.cx(ui.mono, "min-w-0 flex-1 break-all")}>{value}</code>
      <Copy value={value} />
    </div>
  );
  return (
    <div className="grid gap-6">
      <section className={ui.card} aria-label="Status">
        <div className={ui.cardBody}>
          <div className="flex flex-wrap items-center gap-3">
            <span className={ui.badge(mailbox.state === "ready" ? "success" : mailbox.state === "creating" ? "warning" : "danger")}>
              {({ ready: "Ready", creating: "Setting up", failed: "Failed", suspending: "Suspending", suspended: "Suspended", "migration-required": "Operator migration required" } as const)[mailbox.state]}
            </span>
            <span className="text-sm text-muted">{mailbox.name ? `${mailbox.name} <${mailbox.address}>` : mailbox.address}</span>
          </div>
          {mailbox.state === "creating" ? <p className="text-[13px] text-muted">The first mailbox starts the customer’s shared mail server, which takes a moment. This page updates by itself.</p> : null}
          {mailbox.state === "migration-required" ? <p role="alert" className="text-sm text-danger">The retained project mail server needs operator migration. Mail data has been preserved.</p> : null}
          {mailbox.state === "suspended" ? <p className="text-sm text-muted">Incoming mail is suspended for this customer. Mailboxes and stored mail are retained.</p> : null}
          {mailbox.error ? <p role="alert" className="text-sm text-danger">{mailbox.error}</p> : null}
        </div>
      </section>
      {mailbox.state === "ready" && mailbox.host ? (
        <section className={ui.card} aria-label="Mail client settings">
          <h2 className={ui.cx(ui.sectionTitle, "px-6 pt-4 max-sm:px-[18px]")}>Mail client settings</h2>
          <p className="px-6 pb-3 pt-1 text-[13px] text-muted max-sm:px-[18px]">{mailbox.tls}.</p>
          {row("IMAP server", `${mailbox.host}:${String(mailbox.imapPort)}`)}
          {row("SMTP server", `${mailbox.host}:${String(mailbox.smtpPort)}`)}
          {row("Username", mailbox.username)}
          {context.writable ? <MailboxPassword context={context} /> : null}
        </section>
      ) : null}
      {context.writable ? (
        <div className={ui.cx(ui.card, ui.cardBody)}>
          <Remove context={context} action="mailboxes.delete" noun="mailbox" feature="create-mailboxes" />
        </div>
      ) : null}
    </div>
  );
}

function MailboxCreate(props: CreateProps) {
  const create = usePluginMutation(props, props.action);
  const domains = props.resources.filter((resource) => resource.offering === "domain");
  const [domain, setDomain] = useState(domains[0]?.id ?? "");
  const suffix = domains.find((d) => d.id === domain)?.name;
  return (
    <form
      className={ui.cx(ui.card, ui.cardBody, "mb-6")}
      aria-label={props.title}
      onSubmit={(event) => {
        event.preventDefault();
        const data = Object.fromEntries(new FormData(event.currentTarget)) as Record<string, string>;
        create.mutate({ domain_id: domain, local_part: data.local_part, name: data.name }, {
          onSuccess: (result) => {
            const created = createdSchema.safeParse(result);
            if (created.success) props.onCreated(created.data.resource.id);
          },
        });
      }}
    >
      <h2 className={ui.sectionTitle}>{props.title}</h2>
      {domains.length === 0 ? (
        <p className="text-sm text-muted">Add a sending domain first; mailboxes live on your project's domains.</p>
      ) : (
        <>
          <div className="grid gap-4 sm:grid-cols-2">
            <label className={ui.field}>
              Address
              <span className="flex items-center gap-2">
                <input className={ui.input} name="local_part" required maxLength={64} autoComplete="off" />
                <span className="shrink-0 text-sm text-muted">@{suffix}</span>
              </span>
            </label>
            <label className={ui.field}>
              Domain
              <select className={ui.input} value={domain} onChange={(event) => { setDomain(event.target.value); }}>
                {domains.map((d) => <option key={d.id} value={d.id}>{d.name}</option>)}
              </select>
            </label>
          </div>
          <label className={ui.field}>
            Display name
            <input className={ui.input} name="name" maxLength={80} />
          </label>
        </>
      )}
      {create.error ? <p role="alert" className="text-sm text-danger">{create.error.message}</p> : null}
      <div className="flex gap-3">
        <button className={ui.button("primary")} disabled={create.isPending || domains.length === 0}>{create.isPending ? "Creating…" : "Create"}</button>
        <button type="button" className={ui.button()} onClick={props.onCancel}>Cancel</button>
      </div>
    </form>
  );
}

function Resource(context: ResourceProps) {
  if (context.resource.offering === "relay") return <RelayPage {...context} />;
  if (context.resource.offering === "mailbox") return <MailboxPage {...context} />;
  return <DomainPage {...context} />;
}

export default { Resource, Create: { "mailboxes.create": MailboxCreate } } satisfies PluginUI;
