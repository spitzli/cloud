import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useI18n } from "@webdock/i18n/react";
import { useApi } from "./context";
import { AuthHeader, authFrame, authPanel, describeFailure } from "./AuthFrame";
import { waitlistSubmission, waitlistAccepted, waitlistDecision, waitlistEntry, waitlistPage, type WaitlistEntry } from "./waitlist-contracts";
import { Empty, Failure, Loading } from "./common";
import * as ui from "./ui";

export function PublicWaitlist({ confirmed = false }: { confirmed?: boolean }) {
  const api = useApi();
  const { t } = useI18n();
  const [invalid, setInvalid] = useState(false);
  const submitting = useRef(false);
  const submit = useMutation({
    mutationFn: (input: unknown) => api.write("/api/console/waitlist", "POST", waitlistSubmission.parse(input), waitlistAccepted),
    onSuccess: () => { window.location.assign("/waitlist/confirmed"); },
    onSettled: () => { submitting.current = false; },
  });
  return <main className={authFrame}>
    <AuthHeader product="Console" />
    <section className={authPanel}>
      <h1>{confirmed ? t("You're on the waitlist") : t("Get early access")}</h1>
      <p>{confirmed
        ? t("Your request has been saved. We will review it before inviting you to Spitzli Cloud.")
        : t("Spitzli Cloud is currently invitation-only. Leave your contact details to request access.")}</p>
      {confirmed ? <p><a className={ui.textButton} href="https://spitzli.cloud">{t("Back to homepage")}</a></p> : (
        <form className="mt-5 grid gap-5" aria-busy={submit.isPending} onSubmit={event => {
          event.preventDefault();
          if (submitting.current) return;
          const data = new FormData(event.currentTarget);
          const input = waitlistSubmission.safeParse({ email: data.get("email"), name: data.get("name"), privacyAccepted: data.get("privacy") === "on" });
          setInvalid(!input.success);
          if (input.success) { submitting.current = true; submit.mutate(input.data); }
        }}>
          <fieldset disabled={submit.isPending} className="contents">
            <label className={ui.field}>{t("Email")}<input className={ui.input} type="email" name="email" autoComplete="email" required maxLength={254} /></label>
            <label className={ui.field}>{t("Name")} <span className="text-muted">{t("(optional)")}</span><input className={ui.input} name="name" autoComplete="name" maxLength={100} /></label>
            <label className="flex items-start gap-2 text-sm">
              <input type="checkbox" name="privacy" required />
              <span>{t("I agree to the")} <a href="https://spitzli.cloud/privacy" target="_blank" rel="noreferrer">{t("privacy policy")}</a> {t("for processing my access request.")}</span>
            </label>
            {invalid || submit.isError ? <p role="alert" className="text-sm text-danger">{invalid
              ? t("Enter a valid email and accept the privacy policy.")
              : submit.error && t(describeFailure(submit.error))}</p> : null}
            <button className={ui.button("primary")} disabled={submit.isPending}>{submit.isPending ? t("Saving…") : t("Join the waitlist")}</button>
          </fieldset>
        </form>
      )}
      <p className="mt-5 text-sm">{t("Already invited?")} <a className={ui.textButton} href="/login">{t("Sign in")}</a></p>
    </section>
  </main>;
}

export function WaitlistReview() {
  const api = useApi(), cache = useQueryClient();
  const { t, date } = useI18n();
  const [cursor, setCursor] = useState<string>();
  const [filter, setFilter] = useState<WaitlistEntry["status"] | "all">("all");
  const [search, setSearch] = useState("");
  const [rejecting, setRejecting] = useState<string>();
  const reviewing = useRef(false);
  const rejectionTrigger = useRef<HTMLButtonElement | null>(null);
  const rejectionCancel = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    if (rejecting) rejectionCancel.current?.focus();
    else rejectionTrigger.current?.focus();
  }, [rejecting]);
  const list = useQuery({
    queryKey: ["waitlist", cursor],
    queryFn: ({ signal }) => api.get("/api/console/waitlist" + (cursor ? "?cursor=" + encodeURIComponent(cursor) : ""), waitlistPage, signal),
  });
  const review = useMutation({
    mutationFn: ({ id, action, version }: { id: string; action: "approve" | "reject"; version: number }) => api.write("/api/console/waitlist/" + id, "POST", waitlistDecision.parse({ action, version }), waitlistEntry),
    onSettled: async () => {
      try { await cache.invalidateQueries({ queryKey: ["waitlist"] }); }
      finally { reviewing.current = false; }
    },
  });
  const busy = review.isPending || list.isFetching;
  function decide(entry: WaitlistEntry, action: "approve" | "reject") {
    if (reviewing.current || busy) return;
    reviewing.current = true;
    setRejecting(undefined);
    review.mutate({ id: entry.id, action, version: entry.version });
  }
  function changePage(next?: string) {
    review.reset(); setRejecting(undefined); setCursor(next);
  }
  const statuses = { pending: t("Pending"), approved: t("Approved"), rejected: t("Rejected") };
  const entries = list.data?.entries ?? [];
  const term = search.trim().toLocaleLowerCase();
  const visible = entries.filter(entry => (filter === "all" || entry.status === filter)
    && (!term || (entry.email + " " + (entry.name ?? "")).toLocaleLowerCase().includes(term)));
  return <section>
    <header className={ui.pageHeading}>
      <div><h1 className={ui.pageTitle}>{t("Waitlist")}</h1>
        <p className={ui.pageLead}>{t("Review access requests and decide who can start onboarding.")}</p></div>
      <button className={ui.button()} disabled={busy} onClick={() => { setRejecting(undefined); void list.refetch(); }}>{list.isFetching ? t("Refreshing…") : t("Refresh requests")}</button>
    </header>
    {review.isSuccess ? <p role="status" className={ui.cx(ui.notice, "mb-5")}>
      <strong className="block mb-1 [overflow-wrap:anywhere]">{review.data.email}</strong>
      {review.data.status === "approved"
        ? t("Request approved. Invite this contact from workspace member management to grant access.")
        : t("Request rejected.")}</p> : null}
    {review.isError ? <div className="mb-5"><Failure error={review.error} title={t("Unable to review request")} /></div> : null}
    <div className="grid items-start gap-6 xl:grid-cols-[minmax(0,1fr)_280px]">
      <div className="min-w-0 grid gap-4">
        {list.isSuccess ? <>
          <div className="grid grid-cols-3 gap-3" role="group" aria-label={t("Filter by status")}>
            {(["pending", "approved", "rejected"] as const).map(status => <button key={status}
              className={ui.cx(ui.card, "min-w-0 p-4 text-left transition-colors hover:bg-subtle max-sm:p-3", filter === status && "border-foreground bg-subtle")}
              aria-pressed={filter === status} disabled={review.isPending}
              onClick={() => { setFilter(status); setRejecting(undefined); }}>
              <span className="block text-xs text-muted">{statuses[status]}</span>
              <span className="block mt-2 text-2xl font-semibold tabular-nums">{entries.filter(entry => entry.status === status).length}</span>
            </button>)}
          </div>
          <div className="flex flex-wrap items-center gap-3">
            <button className={ui.button(filter === "all" ? "primary" : "secondary")} aria-pressed={filter === "all"} disabled={review.isPending}
              onClick={() => { setFilter("all"); setRejecting(undefined); }}>{t("All requests")}</button>
            <label className="min-w-0 flex-1 basis-56"><span className="sr-only">{t("Search this page")}</span>
              <input className={ui.input} type="search" value={search} placeholder={t("Search by name or email")}
                disabled={review.isPending} onChange={event => { setSearch(event.target.value); setRejecting(undefined); }} /></label>
          </div>
          <p className="text-xs text-muted">{t("Counts and search apply to this page. Use older requests to review more.")}</p>
        </> : null}
        {list.isPending ? <Loading>{t("Loading access requests…")}</Loading>
          : list.isError ? <Failure error={list.error} retry={() => { void list.refetch(); }} />
          : entries.length === 0 ? <Empty title={t("No access requests")}>{t("New requests will appear here.")}</Empty>
          : visible.length === 0 ? <Empty title={t("No matching requests")}>
            {t("Try another name, email or status on this page.")}
            <button className={ui.cx(ui.button(), "block mt-4")} onClick={() => { setSearch(""); setFilter("all"); }}>{t("Clear filters")}</button>
          </Empty>
          : <ul className="grid gap-3" aria-label={t("Access requests")}>{visible.map(entry => (
            <li className={ui.card} key={entry.id}>
              <div className="flex items-start justify-between gap-3 p-5 max-sm:p-4">
                <div className="min-w-0 [overflow-wrap:anywhere]">
                  <p className="font-medium">{entry.name ?? entry.email}</p>
                  {entry.name ? <p className="mt-1 text-sm text-muted">{entry.email}</p> : null}
                  <dl className="mt-4 flex flex-wrap gap-x-6 gap-y-2 text-xs text-muted">
                    <div><dt className="mb-1">{t("Requested")}</dt><dd><time dateTime={entry.createdAt} title={date(entry.createdAt, { dateStyle: "medium", timeStyle: "short" })}>{date(entry.createdAt)}</time></dd></div>
                    {entry.reviewedAt ? <div><dt className="mb-1">{t("Reviewed")}</dt><dd><time dateTime={entry.reviewedAt} title={date(entry.reviewedAt, { dateStyle: "medium", timeStyle: "short" })}>{date(entry.reviewedAt)}</time></dd></div> : null}
                  </dl>
                </div>
                <span className={ui.badge(entry.status === "pending" ? "warning" : entry.status === "approved" ? "success" : "neutral")}>{statuses[entry.status]}</span>
              </div>
              {entry.status === "pending" ? <div className={ui.cardFooter}>
                {rejecting === entry.id ? <div className="w-full" role="group" aria-label={t("Confirm rejection")}>
                  <p className="font-medium text-foreground">{t("Reject this access request?")}</p>
                  <p className="mt-1 mb-3">{t("This contact will not be allowed to start onboarding.")}</p>
                  <div className="flex flex-wrap gap-2">
                    <button className={ui.button("danger")} disabled={busy} onClick={() => { decide(entry, "reject"); }}>{t("Confirm rejection")}</button>
                    <button ref={rejectionCancel} className={ui.button()} disabled={busy} onClick={() => { setRejecting(undefined); }}>{t("Keep pending")}</button>
                  </div>
                </div> : null}
                <div className={rejecting === entry.id ? "hidden" : "flex w-full flex-wrap items-center justify-between gap-3"}>
                  <span>{t("Awaiting your review")}</span>
                  <div className="flex flex-wrap gap-2" role="group" aria-label={entry.email}>
                    <button className={ui.button("secondary")} disabled={busy} onClick={event => { rejectionTrigger.current = event.currentTarget; setRejecting(entry.id); }}>{review.isPending && review.variables.id === entry.id && review.variables.action === "reject" ? t("Rejecting…") : t("Reject")}</button>
                    <button className={ui.button("primary")} disabled={busy} onClick={() => { decide(entry, "approve"); }}>{review.isPending && review.variables.id === entry.id && review.variables.action === "approve" ? t("Approving…") : t("Approve")}</button>
                  </div>
                </div>
              </div> : null}
            </li>
          ))}</ul>}
        <nav className="flex flex-wrap justify-between gap-3" aria-label={t("Request pages")}>
          {cursor ? <button className={ui.button()} disabled={busy} onClick={() => { changePage(); }}>{t("Newest requests")}</button> : <span />}
          {!list.isError && list.data?.nextCursor ? <button className={ui.button()} disabled={busy} onClick={() => { changePage(list.data.nextCursor ?? undefined); }}>{t("Older requests")}</button> : null}
        </nav>
      </div>
      <aside className={ui.card} aria-label={t("How access works")}>
        <div className={ui.cardBody}>
          <h2 className={ui.sectionTitle}>{t("How access works")}</h2>
          <div><h3 className="text-sm font-medium">{t("Approve onboarding")}</h3><p className="mt-1 text-sm text-muted">{t("Approval lets this contact start onboarding. It does not grant access to an existing workspace.")}</p></div>
          <div><h3 className="text-sm font-medium">{t("Assign workspace access")}</h3><p className="mt-1 text-sm text-muted">{t("Invite approved contacts from the workspace member settings when they need to join an existing team.")}</p></div>
          <p className="text-xs text-muted">{t("Review requests individually. Decisions are saved immediately.")}</p>
        </div>
      </aside>
    </div>
  </section>;
}
