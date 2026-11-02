import { useRef, useState } from "react";
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
  const reviewing = useRef(false);
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
    review.mutate({ id: entry.id, action, version: entry.version });
  }
  const statuses = { pending: t("Pending"), approved: t("Approved"), rejected: t("Rejected") };
  return <section>
    <h1>{t("Waitlist")}</h1>
    <p className="mt-2 text-muted">{t("Review access requests. Approval permits onboarding; workspace access is assigned separately.")}</p>
    {review.isSuccess ? <p role="status" className="mt-4 text-sm">{review.data.status === "approved"
      ? t("Request approved. Invite this contact from workspace member management to grant access.")
      : t("Request rejected.")}</p> : null}
    {review.isError ? <Failure error={review.error} title={t("Unable to review request")} /> : null}
    {list.isPending ? <Loading>{t("Loading access requests…")}</Loading>
      : list.isError ? <Failure error={list.error} retry={() => { void list.refetch(); }} />
      : list.data.entries.length === 0 ? <Empty title={t("No access requests")}>{t("New requests will appear here.")}</Empty>
      : <ul className="mt-6 divide-y divide-outline">{list.data.entries.map(entry => (
        <li className="flex flex-wrap items-center justify-between gap-4 py-4 max-sm:flex-col max-sm:items-stretch" key={entry.id}>
          <div className="min-w-0 flex-1 [overflow-wrap:anywhere]">
            <p className="font-medium">{entry.name ?? entry.email}</p>
            <p className="text-sm text-muted">{entry.name ? entry.email + " · " : ""}{statuses[entry.status]} · <time dateTime={entry.createdAt} title={date(entry.createdAt, { dateStyle: "medium", timeStyle: "short" })}>{date(entry.createdAt)}</time></p>
          </div>
          {entry.status === "pending" ? <div className="flex gap-2" role="group" aria-label={entry.email}>
            <button className={ui.button("primary")} disabled={busy} onClick={() => { decide(entry, "approve"); }}>{review.isPending && review.variables.id === entry.id && review.variables.action === "approve" ? t("Approving…") : t("Approve")}</button>
            <button className={ui.button("secondary")} disabled={busy} onClick={() => { decide(entry, "reject"); }}>{review.isPending && review.variables.id === entry.id && review.variables.action === "reject" ? t("Rejecting…") : t("Reject")}</button>
          </div> : null}
        </li>
      ))}</ul>}
    <div className="mt-5 flex gap-3">
      {cursor ? <button className={ui.textButton} disabled={busy} onClick={() => { review.reset(); setCursor(undefined); }}>{t("Newest requests")}</button> : null}
      {!list.isError && list.data?.nextCursor ? <button className={ui.textButton} disabled={busy} onClick={() => { review.reset(); setCursor(list.data.nextCursor ?? undefined); }}>{t("Older requests")}</button> : null}
    </div>
  </section>;
}
