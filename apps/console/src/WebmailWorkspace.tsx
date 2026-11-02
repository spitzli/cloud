import { useCallback, useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useSession } from "./context";
import { Loading } from "./common";
import { foldersSchema, orderFolders, identitiesSchema, listSchema, lookupSchema, messageSchema, submissionSchema, type MailEmail, type MailFolder, type MailSubmission } from "./webmail-contracts";
import { WebmailError, writeMail, type WebmailConnection, type WriteCommand, type WriteOutcome } from "./webmail-client";
import { blankEditor, editorPayload, replyEditor, type MailEditor } from "./webmail-editor";
import { MailComposer, MailMessage, MailMessageList, SubmissionNotice } from "./WebmailPanels";
import * as ui from "./ui";

type WriteLedger = { command: WriteCommand; uncertain: boolean } | null;
export function WebmailWorkspace({ connection, customerID, mailboxID, name, address, sendingAvailable }: {
  connection: WebmailConnection; customerID: string; mailboxID: string; name: string; address: string; sendingAvailable: boolean;
}) {
  const session = useSession();
  const client = useQueryClient();
  const key = ["webmail", session.session.id, customerID, mailboxID, "write"];
  const ledger = useQuery<WriteLedger>({ queryKey: key, queryFn: () => null, initialData: null, enabled: false, gcTime: Infinity });
  const inFlight = useRef(false);
  const [busy, setBusy] = useState(false);
  const [folders, setFolders] = useState<MailFolder[]>([]);
  const [identity, setIdentity] = useState("");
  const [folder, setFolder] = useState("");
  const [search, setSearch] = useState("");
  const [query, setQuery] = useState("");
  const [position, setPosition] = useState(0);
  const [refresh, setRefresh] = useState(0);
  const [selected, setSelected] = useState("");
  const [editor, setEditor] = useState<MailEditor | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [submission, setSubmission] = useState<MailSubmission | null>(null);
  const [submissionEmail, setSubmissionEmail] = useState("");
  const report = useCallback((cause: unknown) => {
    if (connection.isClosed()) return;
    if (cause instanceof WebmailError && cause.denied) {
      connection.deny();
      return;
    }
    setError(cause instanceof WebmailError ? cause.message : "Check the entered values or refresh the mailbox.");
  }, [connection]);
  useEffect(() => {
    const controller = new AbortController();
    void Promise.all([
      connection.call({ action: "folders" }, foldersSchema, controller.signal),
      connection.call({ action: "identities" }, identitiesSchema, controller.signal),
    ]).then(([folders, identities]) => {
      if (controller.signal.aborted || connection.isClosed()) return;
      setFolders(orderFolders(folders.data.list));
      setFolder((current) => folders.data.list.some((item) => item.id === current) ? current : (folders.data.list.find((item) => item.role === "inbox")?.id ?? folders.data.list[0]?.id ?? ""));
      setIdentity(identities.data.find((item) => item.email === address)?.id ?? "");
    }).catch((cause: unknown) => { if (!controller.signal.aborted) report(cause); });
    return () => { controller.abort(); };
  }, [connection, address, refresh, report]);
  const listing = useMessages(connection, folder, query, position, refresh, report);
  const message = useMessage(connection, selected, refresh, report);
  const outcome = (value: WriteOutcome) => {
    if (value.kind === "draft") {
      const signature = JSON.stringify(value.command.draft);
      setEditor((current) => {
        if (!current) return current;
        return { ...current, previousId: value.value.id, savedSignature: signature };
      });
      setNotice("Draft saved.");
    }
    if (value.kind === "update") setNotice("Message change recorded.");
    if (value.kind === "submission") {
      setSubmission(value.value);
      if (value.command.action === "submit") setSubmissionEmail(value.command.emailId);
      if (["accepted", "queued", "leased", "handoff", "staging"].includes(value.value.state)) setEditor(null);
    }
    setRefresh((current) => current + 1);
  };
  const runWrite = async (command: WriteCommand, exactRetry = false) => {
    if (inFlight.current || (ledger.data && !exactRetry) || connection.isClosed()) return;
    if (exactRetry && JSON.stringify(command) !== JSON.stringify(ledger.data?.command)) return;
    inFlight.current = true;
    setBusy(true); setError(""); setNotice("");
    client.setQueryData<WriteLedger>(key, () => ({ command, uncertain: false }));
    try {
      const result = await writeMail(connection, command);
      if (connection.isClosed()) return;
      client.setQueryData<WriteLedger>(key, null);
      outcome(result);
    } catch (cause) {
      if (connection.isClosed()) return;
      const uncertain = cause instanceof WebmailError && cause.uncertain;
      client.setQueryData<WriteLedger>(key, () => uncertain ? { command, uncertain: true } : null);
      report(cause);
    } finally {
      inFlight.current = false;
      if (!connection.isClosed()) setBusy(false);
    }
  };
  const save = () => {
    if (!editor) return;
    try {
      const draft = editorPayload(editor);
      void runWrite({ action: "draft", requestID: crypto.randomUUID(), draft, ...(editor.previousId ? { previousId: editor.previousId } : {}) });
    } catch (cause) { report(cause); }
  };
  const saved = (() => {
    if (!editor?.previousId) return false;
    try { return editor.savedSignature === JSON.stringify(editorPayload(editor)); } catch { return false; }
  })();
  const send = () => {
    if (!sendingAvailable || !saved || !editor?.previousId) return;
    void runWrite({ action: "submit", requestID: crypto.randomUUID(), emailId: editor.previousId, identityId: editor.identityId });
  };
  const refreshSubmission = async (operation: "status" | "cancel" | "file") => {
    if (!submission) return;
    if (operation !== "status") { await runWrite({ action: "submission", operation, id: submission.id }); return; }
    try {
      const result = await connection.call({ action: "submission", operation, id: submission.id }, submissionSchema);
      if (!connection.isClosed()) setSubmission(result.data);
    } catch (cause) { report(cause); }
  };
  const lookup = async () => {
    const emailId = ledger.data?.command.action === "submit" ? ledger.data.command.emailId : submissionEmail;
    if (!emailId) return;
    try {
      const result = await connection.call({ action: "submission-lookup", emailId }, lookupSchema);
      if (connection.isClosed()) return;
      if (result.data) {
        setSubmission(result.data); setSubmissionEmail(emailId);
        if (ledger.data?.command.action === "submit") client.setQueryData<WriteLedger>(key, null);
      }
      if (!result.data) setNotice("No submission is recorded yet. Do not send a new request while the outcome is unknown.");
    } catch (cause) { report(cause); }
  };
  const upload = async (files: FileList | null) => {
    if (!files?.length || !editor || inFlight.current || ledger.data) return;
    if (editor.attachments.length + files.length > 10 || editor.attachments.reduce((sum, item) => sum + item.size, 0) + [...files].reduce((sum, file) => sum + file.size, 0) > 10485760) {
      setError("A draft supports at most 10 attachments and 10 MiB total."); return;
    }
    inFlight.current = true; setBusy(true);
    try {
      for (const file of files) {
        const attachment = await connection.upload(file);
        if (connection.isClosed()) return;
        setEditor((current) => current ? { ...current, attachments: [...current.attachments, attachment] } : null);
      }
    } catch (cause) { report(cause); }
    finally { inFlight.current = false; if (!connection.isClosed()) setBusy(false); }
  };
  const openEditor = (reply?: MailEmail) => {
    if (!identity || busy || ledger.data) return;
    setEditor(reply ? replyEditor(reply, identity) : blankEditor(identity));
  };
  const blocked = busy || ledger.data !== null;
  return <section aria-label="Webmail" className="grid gap-3">
    <header className={ui.cx(`${ui.pageHeading} mb-3 max-sm:mb-3`)}>
      <div className="min-w-0 [overflow-wrap:anywhere]"><Link to="/mail" search={{ customer: customerID }} className="text-[13px] text-muted hover:text-foreground">Mailboxes</Link><h1 className={ui.cx(`${ui.pageTitle} mt-2`)}>{name}</h1><p className={ui.pageLead}>{address}</p></div>
      <button className={ui.button()} onClick={() => { setRefresh((value) => value + 1); }}>Refresh mailbox</button>
    </header>
    {error ? <p className={ui.cx(`${ui.notice} text-danger [overflow-wrap:anywhere]`)} role="alert">{error}</p> : null}
    {notice ? <p className={ui.notice} role="status">{notice}</p> : null}
    {ledger.data ? <section className={ui.cx(`${ui.notice} grid justify-items-start gap-2`)} role="status">
      <strong className="font-semibold">{busy ? "Mail change in progress" : "Mail change outcome is unknown"}</strong>
      <p>Review the mailbox before another change. Only this exact saved request can be retried; a retry never creates a new request identifier.</p>
      <button className={ui.button()} disabled={busy} onClick={() => { if (ledger.data) void runWrite(ledger.data.command, true); }}>Retry same request</button>
      {ledger.data.command.action === "submit" ? <button className={ui.button()} disabled={busy} onClick={() => void lookup()}>Check sending status</button> : null}
    </section> : null}
    {submission ? <SubmissionNotice value={submission} blocked={blocked} change={(operation) => void refreshSubmission(operation)} /> : null}
    <div className="mt-1 grid min-h-[620px] grid-cols-[minmax(0,1fr)] overflow-hidden rounded-xl border border-outline bg-surface min-[601px]:grid-cols-[140px_minmax(0,1fr)] min-[1101px]:grid-cols-[160px_minmax(220px,.8fr)_minmax(280px,1.4fr)]">
      <aside className="border-outline bg-subtle px-2.5 py-4 max-[600px]:border-b min-[601px]:border-r">
        <button className={ui.cx(`${ui.button("primary")} mb-4 w-full`)} disabled={blocked || !identity || Boolean(editor)} onClick={() => { openEditor(); }}>New message</button>
        <nav aria-label="Mail folders" className="max-[600px]:flex max-[600px]:flex-wrap max-[600px]:gap-1">{folders.map((item) => <button key={item.id} className="flex w-full justify-between gap-2.5 rounded-md px-2.5 py-2 text-left text-sm text-foreground [overflow-wrap:anywhere] hover:bg-soft aria-[current=page]:bg-soft aria-[current=page]:font-semibold max-[600px]:w-auto" aria-current={folder === item.id ? "page" : undefined} onClick={() => { setFolder(item.id); setPosition(0); setSelected(""); }}>
          <span>{item.name}</span><span className="text-muted">{item.unreadEmails || ""}</span>
        </button>)}</nav>
      </aside>
      <div className="flex min-w-0 flex-col border-outline min-[1101px]:border-r">
        <form className="flex gap-1.5 border-b border-outline p-3" onSubmit={(event) => { event.preventDefault(); setQuery(search); setPosition(0); setSelected(""); }}>
          <label className="sr-only" htmlFor="mail-search">Search messages</label><input id="mail-search" className={ui.cx(`${ui.input} min-h-9`)} value={search} maxLength={500} placeholder="Search messages" onChange={(event) => { setSearch(event.target.value); }} />
          <button className={ui.button()} type="submit">Search</button>
        </form>
        {listing ? <MailMessageList value={listing} selected={selected} select={setSelected} position={position} page={setPosition} /> : <Loading>Loading messages…</Loading>}
      </div>
      <div className="min-w-0 border-outline max-[1100px]:col-span-full max-[1100px]:border-t">
        {editor ? <MailComposer value={editor} change={setEditor} save={save} send={send} close={() => { if (!blocked && window.confirm("Close this composer? Unsaved changes will be discarded.")) setEditor(null); }} upload={(files) => void upload(files)} blocked={blocked} saved={saved} sendingAvailable={sendingAvailable} /> :
          <MailMessage message={message} selected={selected} connection={connection} folders={folders} blocked={blocked} reply={identity ? (email) => { openEditor(email); } : undefined} update={(emailId, change) => void runWrite({ action: "update", emailId, change, requestID: crypto.randomUUID() })} report={report} />}
      </div>
    </div>
  </section>;
}
function useMessages(connection: WebmailConnection, folder: string, text: string, position: number, refresh: number, report: (error: unknown) => void) {
  const key = JSON.stringify([folder, text, position, refresh]);
  const [snapshot, setSnapshot] = useState<{ key: string; value: { list: MailEmail[]; total: number; position: number } } | null>(null);
  useEffect(() => {
    if (!folder) return;
    const controller = new AbortController();
    void connection.call({ action: "list", mailboxId: folder, text, position, limit: 50 }, listSchema, controller.signal).then((value) => {
      if (!controller.signal.aborted && !connection.isClosed()) setSnapshot({ key, value: value.data });
    }).catch((cause: unknown) => { if (!controller.signal.aborted) report(cause); });
    return () => { controller.abort(); };
  }, [connection, folder, text, position, key, report]);
  return snapshot?.key === key ? snapshot.value : null;
}
function useMessage(connection: WebmailConnection, selected: string, refresh: number, report: (error: unknown) => void) {
  const key = JSON.stringify([selected, refresh]);
  const [snapshot, setSnapshot] = useState<{ key: string; value: MailEmail } | null>(null);
  useEffect(() => {
    if (!selected) return;
    const controller = new AbortController();
    void connection.call({ action: "read", emailId: selected }, messageSchema, controller.signal).then((value) => {
      if (value.data.id !== selected) throw new WebmailError("Mail returned a different message.");
      if (!controller.signal.aborted && !connection.isClosed()) setSnapshot({ key, value: value.data });
    }).catch((cause: unknown) => { if (!controller.signal.aborted) report(cause); });
    return () => { controller.abort(); };
  }, [connection, selected, key, report]);
  return snapshot?.key === key ? snapshot.value : null;
}
