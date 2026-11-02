import { useRef, useState } from "react";
import { useBlocker } from "@tanstack/react-router";
import { z } from "zod";
import { Failure, Loading, ui, usePluginMutation, usePluginQuery, usePluginSubmission, type PluginSubmission, type ResourceProps } from "@webdock/plugin-ui";

const stateSchema = z.enum(["staging", "queued", "leased", "preparation_review", "handoff", "unknown", "accepted", "rejected", "cancelled"]);
type SendState = z.infer<typeof stateSchema>;
const receiptSchema = z.object({ id: z.string().regex(/^[1-9][0-9]{0,18}$/), state: stateSchema }).strict();
const historySchema = z.object({ items: z.array(receiptSchema.extend({ from: z.string().min(1).max(254), to: z.array(z.string().min(1).max(254)).min(1).max(10), subject: z.string().max(800), transport: z.string().max(80), at: z.string().max(80) })).max(20) }).strict();
const labels: Record<SendState, string> = { staging: "Preparing", queued: "Queued", leased: "Preparing", preparation_review: "Preparation needs review", handoff: "Sending", unknown: "Delivery unconfirmed", accepted: "Accepted by mail service", rejected: "Rejected by mail service", cancelled: "Cancelled" };
const cancellable = new Set<SendState>(["staging", "queued", "leased", "preparation_review"]);
const active = new Set<SendState>(["staging", "queued", "leased", "handoff"]);

export function SendForm({ context, domain }: { context: ResourceProps; domain: { domain: string; verified: boolean } }) {
  if (!["mail.history", "mail.status", "mail.cancel"].every(action => context.actions?.includes(action)))
    return <p className={ui.notice}>This Mail release needs an update before messages can be queued safely.</p>;
  return <DurableSendForm key={`${context.projectID}:${context.resource.id}`} context={context} domain={domain} />;
}

function DurableSendForm({ context, domain }: { context: ResourceProps; domain: { domain: string; verified: boolean } }) {
  const form = useRef<HTMLFormElement>(null);
  const send = usePluginSubmission(context, "mail.send", receiptSchema);
  const [notice, setNotice] = useState<string>();
  // An action can lose authority after durable admission but before returning its receipt.
  const uncertain = send.isError;
  const locked = send.isPending || uncertain;
  useBlocker({ shouldBlockFn: () => locked && !window.confirm("This message may already be queued. Leave and check the outbox before submitting it again?"), enableBeforeUnload: locked });
  const recorded = (receipt: z.infer<typeof receiptSchema>, submission: PluginSubmission) => {
    const to = String(submission.input.to);
    setNotice(receipt.state === "queued" ? `Queued for ${to}.` : `Message recorded for ${to}. Check its outbox status.`);
    form.current?.reset();
    send.reset();
  };
  return (
    <form ref={form} className={ui.cx(ui.card, ui.cardBody)} aria-label="Send a message" onSubmit={event => {
      event.preventDefault();
      if (locked) return;
      setNotice(undefined);
      const input = { resource_id: context.resource.id, ...Object.fromEntries(new FormData(event.currentTarget)) };
      send.mutate({ input, idempotencyKey: crypto.randomUUID() }, { onSuccess: recorded });
    }}>
      <h2 className={ui.sectionTitle}>Send a message</h2>
      <fieldset disabled={locked} className="grid min-w-0 gap-4">
        <div className="grid gap-4 sm:grid-cols-2">
          <label className={ui.field}>From<span className="flex items-center gap-2"><input className={ui.input} name="from" required maxLength={64} defaultValue="hello" /><span className="shrink-0 text-sm text-muted">@{domain.domain}</span></span></label>
          <label className={ui.field}>Sender name<input className={ui.input} name="from_name" maxLength={80} /></label>
        </div>
        <label className={ui.field}>To<input className={ui.input} name="to" required maxLength={2000} placeholder="someone@example.com, another@example.com" /><span className={ui.hint}>Up to 10 addresses, separated by commas.</span></label>
        <label className={ui.field}>Subject<input className={ui.input} name="subject" required maxLength={200} /></label>
        <label className={ui.field}>Message<textarea className={ui.cx(ui.input, "min-h-32")} name="text" required maxLength={30000} /></label>
      </fieldset>
      <div className="flex flex-wrap items-center gap-3">
        <button className={ui.button("primary")} disabled={locked || !domain.verified}>{send.isPending ? "Queuing…" : "Send"}</button>
        {!domain.verified ? <span className="text-[13px] text-muted">Verify the domain first.</span> : null}
        {notice ? <span role="status" className="text-[13px] text-success">{notice}</span> : null}
      </div>
      {send.error ? <div role="alert" className="grid gap-2 text-sm">
        <p className="text-danger">No queue receipt was received. This message may already be queued; retry the same submission to check.</p>
        {uncertain ? <button type="button" className={ui.cx(ui.button(), "justify-self-start")} onClick={() => { send.mutate(send.variables, { onSuccess: recorded }); }}>Retry same message</button> : null}
        <button type="button" className={ui.cx(ui.button("quiet"), "justify-self-start")} onClick={() => { if (window.confirm("The previous message may already be queued. Check the outbox first. Start a separate submission?")) send.reset(); }}>Start a separate message</button>
      </div> : null}
    </form>
  );
}

export function Outbox({ context }: { context: ResourceProps }) {
  if (!context.actions?.includes("mail.history")) return null;
  return <OutboxMessages key={`${context.projectID}:${context.resource.id}`} context={context} />;
}

function OutboxMessages({ context }: { context: ResourceProps }) {
  const query = usePluginQuery(context, "mail.history", { resource_id: context.resource.id }, historySchema, { refetchInterval: data => data?.items.some(item => active.has(item.state)) ? 2000 : false });
  const cancel = usePluginMutation(context, "mail.cancel");
  return (
    <section className={ui.card} aria-label="Outbox">
      <div className={ui.cx(ui.cardRow, "flex-wrap")}><h2 className={ui.sectionTitle}>Outbox</h2><button type="button" className={ui.button("quiet")} onClick={() => void query.refetch()}>Refresh outbox</button></div>
      {query.isPending ? <div className={ui.cardBody}><Loading>Loading messages…</Loading></div> : query.error ? <div className={ui.cardBody}><Failure error={query.error} retry={() => void query.refetch()} /></div> : query.data.items.length === 0 ? <p className="px-6 pb-5 text-sm text-muted max-sm:px-[18px]">No messages queued.</p> : (
        <ul aria-label="Messages">
          {query.data.items.map(message => (
            <li key={message.id} className={ui.cx(ui.cardRow, "flex-wrap items-start gap-3")}>
              <div className="grid min-w-0 flex-1 basis-48 gap-1">
                <span className="break-words font-medium">{message.subject}</span>
                <span className="break-all text-[13px] text-muted">{message.from} to {message.to.join(', ')}</span>
                <span className="text-[12px] text-muted">{new Date(message.at).toLocaleString()}</span>
                {message.state === "unknown" ? <p className="text-[13px] text-muted">The mail service has not confirmed the outcome. This message will not be sent again automatically.</p> : null}
                {message.state === "preparation_review" ? <p className="text-[13px] text-muted">Preparation could not finish safely. Cancel this message or ask an administrator to review it.</p> : null}
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <span className={message.state === "accepted" ? ui.badge("success") : message.state === "rejected" ? ui.badge("danger") : ["unknown", "preparation_review"].includes(message.state) ? ui.badge("warning") : ui.badge()}>{labels[message.state]}</span>
                {context.writable && context.actions?.includes("mail.cancel") && cancellable.has(message.state) ? <button type="button" className={ui.button("quiet")} disabled={cancel.isPending} onClick={() => { cancel.mutate({ resource_id: context.resource.id, id: message.id }); }}>Cancel queued message</button> : null}
              </div>
            </li>
          ))}
        </ul>
      )}
      {cancel.error ? <div className={ui.cardBody}><p role="alert" className="text-sm text-danger">{cancel.error.message}</p></div> : null}
    </section>
  );
}
