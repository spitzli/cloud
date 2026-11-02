import type { Dispatch, SetStateAction } from "react";
import { useMemo } from "react";
import DOMPurify from "dompurify";
import { displayAddresses, messageBody, type MailEmail, type MailFolder, type MailSubmission } from "./webmail-contracts";
import type { WebmailConnection, WriteCommand } from "./webmail-client";
import type { MailEditor } from "./webmail-editor";
import { Loading } from "./common";
import * as ui from "./ui";
const empty = "px-5 py-8 text-muted";

export function MailMessageList({ value, selected, select, position, page }: {
  value: { list: MailEmail[]; total: number }; selected: string; select: (id: string) => void; position: number; page: (position: number) => void;
}) {
  return <>
    <div className="flex-1" aria-label="Messages">
      {value.list.length ? value.list.map((email) => <button className={`grid w-full gap-1 border-b border-outline px-4 py-3.5 text-left text-foreground [overflow-wrap:anywhere] hover:bg-subtle ${selected === email.id ? "bg-soft shadow-[inset_3px_0_var(--color-accent)]" : "bg-surface"}`} key={email.id} aria-pressed={selected === email.id} onClick={() => { select(email.id); }}>
        <span className="truncate text-xs">{displayAddresses(email.from) || "Unknown sender"}</span>
        <strong className="text-[13px] font-semibold">{email.subject || "(No subject)"}</strong><span className="truncate text-xs text-muted">{email.preview}</span>
      </button>) : <p className={empty}>No messages in this view.</p>}
    </div>
    <div className="flex flex-wrap items-center justify-between gap-2 p-3 text-xs"><button className={ui.button()} disabled={position === 0} onClick={() => { page(Math.max(0, position - 50)); }}>Previous</button><span className="text-muted">{value.total ? `${String(position + 1)}–${String(Math.min(position + 50, value.total))} of ${String(value.total)}` : "0 messages"}</span><button className={ui.button()} disabled={position + 50 >= value.total} onClick={() => { page(position + 50); }}>Next</button></div>
  </>;
}
export function MailMessage({ message, selected, connection, folders, blocked, reply, update, report }: {
  message: MailEmail | null; selected: string; connection: WebmailConnection; folders: MailFolder[]; blocked: boolean;
  reply: ((message: MailEmail) => void) | undefined;
  update: (emailId: string, change: Extract<WriteCommand, { action: "update" }>["change"]) => void;
  report: (error: unknown) => void;
}) {
  if (!selected) return <p className={empty}>Select a message to read it.</p>;
  if (!message) return <Loading>Loading message…</Loading>;
  const plain = messageBody(message, "textBody");
  const html = messageBody(message, "htmlBody");
  const download = async (blobId: string, name: string) => {
    try {
      const blob = await connection.download(message.id, blobId);
      if (connection.isClosed()) return;
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url; anchor.download = name; anchor.rel = "noopener";
      anchor.click();
      setTimeout(() => { URL.revokeObjectURL(url); }, 1000);
    } catch (cause) { report(cause); }
  };
  return <article className="p-6 max-sm:p-4" aria-label="Message content">
    <header className="grid gap-1 text-[13px] [overflow-wrap:anywhere]"><h2 className="mb-1.5 text-[19px] font-semibold tracking-[-0.01em]">{message.subject || "(No subject)"}</h2><p>{displayAddresses(message.from)}</p><p className="text-muted">To: {displayAddresses(message.to)}</p>{message.receivedAt ? <p className="text-muted">{message.receivedAt}</p> : null}</header>
    <div className="my-5 flex flex-wrap items-center gap-2 border-b border-outline pb-4">
      <button className={ui.button()} disabled={blocked || !reply} onClick={() => { reply?.(message); }}>Reply</button>
      <button className={ui.button()} disabled={blocked} onClick={() => { update(message.id, { seen: !message.keywords.$seen }); }}>{message.keywords.$seen ? "Mark unread" : "Mark read"}</button>
      <button className={ui.button()} disabled={blocked} onClick={() => { update(message.id, { flagged: !message.keywords.$flagged }); }}>{message.keywords.$flagged ? "Unstar" : "Star"}</button>
      <label className="flex items-center gap-1.5 whitespace-nowrap text-xs">Move to<select className={ui.cx(`${ui.input} min-h-9 w-auto max-w-[150px]`)} aria-label="Move message to folder" disabled={blocked} value="" onChange={(event) => { if (folders.some((folder) => folder.id === event.target.value)) update(message.id, { mailboxIds: [event.target.value] }); }}><option value="">Choose folder</option>{folders.map((folder) => <option key={folder.id} value={folder.id}>{folder.name}</option>)}</select></label>
    </div>
    {plain ? <div className="whitespace-pre-wrap text-sm leading-[1.8] [overflow-wrap:anywhere]">{plain}</div> : html ? <MessageHTML html={html} /> : <p className={empty}>This message has no readable body.</p>}
    {Object.values(message.bodyValues).some((part) => part.isTruncated) ? <p className={ui.cx(`${ui.notice} mt-4`)}>This message body is truncated.</p> : null}
    {message.attachments.length ? <section className="mt-6 grid justify-items-start gap-2"><h3 className="text-sm font-semibold">Attachments</h3>{message.attachments.map((attachment) => <button key={attachment.blobId} className={ui.button()} onClick={() => void download(attachment.blobId, attachment.name)}>{attachment.name} · {Math.ceil(attachment.size / 1024)} KiB</button>)}</section> : null}
  </article>;
}
export function MessageHTML({ html }: { html: string }) {
  // Adapted from the owned Mail viewer. No remote images, links, scripts, forms or document styles.
  const document = useMemo(() => {
    const clean = DOMPurify.sanitize(html, {
      ALLOWED_TAGS: ["p","br","div","span","strong","b","em","i","u","s","blockquote","pre","code","hr","h1","h2","h3","h4","ul","ol","li","table","thead","tbody","tfoot","tr","td","th","caption"],
      ALLOWED_ATTR: ["colspan","rowspan","dir","lang"], ALLOW_DATA_ATTR: false,
    });
    return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"><style>body{font:15px/1.65 Arial,sans-serif;color:#202428;overflow-wrap:anywhere;margin:12px}table{max-width:100%;border-collapse:collapse}td,th{padding:6px}pre{white-space:pre-wrap}blockquote{border-left:2px solid #dfe3e6;padding-left:16px}</style></head><body>${clean}</body></html>`;
  }, [html]);
  return <div><p className="text-[13px] text-muted">External images and active content are blocked.</p><iframe className="mt-3 min-h-[420px] w-full rounded-md border border-outline bg-white" title="Sanitized message content" sandbox="" referrerPolicy="no-referrer" srcDoc={document} /></div>;
}
export function MailComposer({ value, change, save, send, close, upload, blocked, saved, sendingAvailable }: {
  value: MailEditor; change: Dispatch<SetStateAction<MailEditor | null>>; save: () => void; send: () => void; close: () => void; upload: (files: FileList | null) => void; blocked: boolean; saved: boolean; sendingAvailable: boolean;
}) {
  const field = (name: "to" | "cc" | "bcc" | "subject" | "text", value: string) => { change((current) => current ? { ...current, [name]: value } : null); };
  return <form className="p-6 max-sm:p-4" aria-label="Compose message" onSubmit={(event) => { event.preventDefault(); save(); }}>
    <div className="flex items-center justify-between gap-3"><h2 className={ui.sectionTitle}>Compose message</h2><button className={ui.button("quiet")} type="button" disabled={blocked} onClick={close}>Close composer</button></div>
    <fieldset disabled={blocked} className="m-0 grid min-w-0 gap-3 border-0 px-0 pt-4 pb-3">
      {(["to", "cc", "bcc"] as const).map((name) => <label key={name} className={ui.field}>{name === "to" ? "To" : name.toUpperCase()}<input className={ui.input} value={value[name]} maxLength={32000} onChange={(event) => { field(name, event.target.value); }} /></label>)}
      <label className={ui.field}>Subject<input className={ui.input} value={value.subject} maxLength={998} onChange={(event) => { field("subject", event.target.value); }} /></label>
      <label className={ui.field}>Message<textarea className={ui.cx(`${ui.input} resize-y`)} value={value.text} rows={13} onChange={(event) => { field("text", event.target.value); }} /></label>
      <label className={ui.field}>Attach files<input className="w-full min-w-0 rounded-md border border-dashed border-outline-strong p-2 text-sm font-normal text-muted file:mr-3 file:rounded file:border-0 file:bg-soft file:px-2.5 file:py-1 file:text-foreground" type="file" multiple onChange={(event) => { upload(event.target.files); event.target.value = ""; }} /></label>
      {value.attachments.map((attachment) => <div className="flex items-center justify-between gap-2" key={attachment.blobId}><span className="min-w-0 text-sm [overflow-wrap:anywhere]">{attachment.name}</span><button className={ui.button("quiet")} type="button" onClick={() => { change((current) => current ? { ...current, attachments: current.attachments.filter((item) => item.blobId !== attachment.blobId) } : null); }}>Remove attachment</button></div>)}
      <div className="flex flex-wrap gap-2"><button className={ui.button()} type="submit">Save draft</button><button className={ui.button("primary")} type="button" disabled={!saved || !sendingAvailable} onClick={send}>Send message</button></div>
    </fieldset>
    <p className="text-[13px] text-muted">{saved ? (sendingAvailable ? "Draft saved. Sending uses the recorded native queue." : "Draft saved.") : "Save this draft before sending. Unsaved changes stay only in this tab."}</p>
    {!sendingAvailable ? <p className={ui.cx(`${ui.notice} mt-3`)}>Sending is not enabled for this workspace. You can save a draft.</p> : null}
  </form>;
}
export function SubmissionNotice({ value, blocked, change }: {
  value: MailSubmission; blocked: boolean; change: (operation: "status" | "cancel" | "file") => void;
}) {
  const text = {
    staging: "Message is being staged.", queued: "Message is queued.", leased: "Delivery is being prepared.", handoff: "Delivery handoff is in progress.",
    accepted: "Message was accepted for delivery.", rejected: "Message was rejected. The draft is retained.", cancelled: "Sending was cancelled. The draft is retained.",
    unknown: "Delivery outcome is unknown. Do not resend this message.", preparation_review: "Sending needs review. The draft is retained.",
  };
  return <section className={ui.cx(`${ui.notice} grid gap-2`)} role="status"><p>{text[value.state]}</p>
    {value.state === "accepted" ? <p>{value.filed ? "A Sent copy is recorded." : "The Sent copy is not yet confirmed."}</p> : null}
    <div className="flex flex-wrap gap-2"><button className={ui.button()} onClick={() => { change("status"); }}>Check sending status</button>
      {["staging", "queued", "leased", "preparation_review"].includes(value.state) ? <button className={ui.button()} disabled={blocked} onClick={() => { change("cancel"); }}>Cancel sending</button> : null}
      {value.state === "accepted" && !value.filed ? <button className={ui.button()} disabled={blocked} onClick={() => { change("file"); }}>Retry Sent filing</button> : null}
    </div>
  </section>;
}
