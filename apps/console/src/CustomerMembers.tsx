import { useRef, useState, type ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { useApi } from "./context";
import { Failure, Loading, Modal } from "./common";
import { formText } from "./form";
import { FieldError, useLiveValidation } from "./live-validation";
import * as ui from "./ui";

const tenantSchema = z.object({
  canManage: z.boolean(),
  operator: z.boolean().default(false),
  tenant: z.object({ member_role: z.string().nullish() }).default({}),
  members: z.array(z.object({ id: z.string(), role: z.string(), name: z.string(), email: z.string(), mustChangePassword: z.boolean().nullish() })),
  invitations: z.array(z.object({
    id: z.string(), email: z.string(), role: z.string(), status: z.string(), name: z.string().nullish(),
    createdAt: z.string().nullish(), expired: z.boolean().nullish(),
    progress: z.object({ emailDelivered: z.string().nullish(), accountReady: z.boolean().nullish(), joinedAt: z.string().nullish() }).nullish(),
  })),
  activity: z.array(z.object({ id: z.string(), action: z.string(), outcome: z.string(), createdAt: z.string(), actorName: z.string().nullish(), actorEmail: z.string().nullish() })).default([]),
});
type Invitation = z.infer<typeof tenantSchema>["invitations"][number];
const changeSchema = z.object({ message: z.string(), delivery: z.string().nullish(), link: z.string().nullish() });
const deliveryText: Record<string, string> = {
  queued: "The email is on its way.",
  "activation-recently-sent": "A setup email was sent less than a minute ago; try again shortly for a new link.",
  "invitation-queued-activation-unavailable": "The invitation is saved, but the setup email could not be queued.",
};

const actionText: Record<string, string> = {
  "tenant-invite": "Invited",
  "tenant-resend": "Resent invitation",
  "tenant-cancel": "Canceled invitation",
  "accept-invitation": "Joined",
  "reject-invitation": "Declined",
  "tenant-role": "Changed role",
  "tenant-remove": "Removed member",
  "tenant-profile": "Updated customer details",
};
const relative = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
function ago(value: string) {
  const seconds = (new Date(value).getTime() - Date.now()) / 1000;
  for (const [unit, size] of [["year", 31536000], ["month", 2592000], ["day", 86400], ["hour", 3600], ["minute", 60]] as const)
    if (Math.abs(seconds) >= size) return relative.format(Math.round(seconds / size), unit);
  return "just now";
}
function statusOf(invitation: Invitation): [string, Parameters<typeof ui.badge>[0]] {
  if (invitation.status === "accepted") return ["Joined", "success"];
  if (invitation.status === "canceled") return ["Canceled", "neutral"];
  if (invitation.status === "rejected") return ["Declined", "neutral"];
  return invitation.expired ? ["Expired", "danger"] : ["Pending", "warning"];
}
/** Invited → Email delivered → Account set up → Joined; a later step implies the earlier ones. */
function Steps({ invitation }: { invitation: Invitation }) {
  const progress = invitation.progress ?? {};
  const joined = invitation.status === "accepted";
  const ready = joined || Boolean(progress.accountReady);
  const delivered = ready || progress.emailDelivered === "delivered";
  const open = invitation.status === "pending" && !invitation.expired;
  const steps = [
    { label: "Invited", done: true, time: invitation.createdAt },
    { label: "Email delivered", done: delivered, failed: !delivered && progress.emailDelivered === "failed" },
    { label: "Account set up", done: ready },
    { label: "Joined", done: joined, time: progress.joinedAt },
  ];
  const current = open ? steps.findIndex((step) => !step.done) : -1;
  return (
    <ol aria-label={`Progress of ${invitation.email}`} className="flex w-full flex-wrap items-center gap-x-1 gap-y-1 text-xs">
      {steps.map((step, index) => {
        const state = step.done ? "done" : step.failed ? "failed" : index === current ? "current" : "pending";
        return (
          <li key={step.label} data-step={step.label} data-state={state} title={step.time ? new Date(step.time).toLocaleString() : undefined}
            className={`flex items-center gap-1.5 ${state === "pending" ? "text-muted" : "text-foreground"}`}>
            {index > 0 ? <span aria-hidden className={`h-px w-4 ${steps[index - 1]?.done ? "bg-success" : "bg-outline-strong"}`} /> : null}
            <span aria-hidden className={`size-2 shrink-0 rounded-full ${{ done: "bg-success", failed: "bg-danger", current: "bg-accent ring-3 ring-ring", pending: "border border-outline-strong" }[state]}`} />
            {step.label}
            {step.time ? <span className="text-muted max-lg:hidden">{ago(step.time)}</span> : null}
            <span className="sr-only">{{ done: "(done)", failed: "(failed)", current: "(current step)", pending: "(not yet)" }[state]}</span>
          </li>
        );
      })}
    </ol>
  );
}

/** After creating a customer: invite the first person, prefilled from the contact just entered. */
export interface FirstInvite { email: string; name: string; done: () => void }

/** People with access to this customer, their invitations and what happened to them. */
export function CustomerMembers({ customer, welcome }: { customer: string; welcome?: FirstInvite | undefined }) {
  const api = useApi();
  const client = useQueryClient();
  const live = useLiveValidation();
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState("");
  const linkInput = useRef<HTMLInputElement>(null);
  const [removing, setRemoving] = useState<{ id: string; name: string; email: string }>();
  const path = `/api/console/tenant/${encodeURIComponent(customer)}`;
  const tenant = useQuery({ queryKey: ["tenant", customer], queryFn: ({ signal }) => api.request(path, tenantSchema, { signal }) });
  const change = useMutation({
    mutationFn: (command: Record<string, unknown>) => api.write(path, "POST", command, changeSchema),
    onSuccess: () => { setCopied(false); setCopyError(""); void client.invalidateQueries({ queryKey: ["tenant", customer] }); },
  });
  const copyLink = async (link: string) => {
    setCopyError("");
    try {
      await navigator.clipboard.writeText(link);
      setCopied(true);
    } catch {
      linkInput.current?.focus();
      linkInput.current?.select();
      setCopyError("Automatic copying is unavailable. Select the link and copy it manually.");
    }
  };
  if (tenant.isPending) return <Loading>Loading members…</Loading>;
  if (tenant.error) return <Failure error={tenant.error} retry={() => { void tenant.refetch(); }} />;
  const result = change.data;
  const busy = !tenant.data.canManage || change.isPending;
  // Operators and owners manage roles, including ownership; the server keeps at least one owner.
  const canSetRoles = tenant.data.canManage && (tenant.data.operator || tenant.data.tenant.member_role === "owner");
  // Until someone owns the workspace (or is invited to), the next invitation offers ownership first.
  const needsOwner = canSetRoles && !tenant.data.members.some((member) => member.role === "owner")
    && !tenant.data.invitations.some((invitation) => invitation.role === "owner" && invitation.status === "pending" && !invitation.expired);
  // Everything that still needs attention, plus the ten newest answered invitations; older ones fold away.
  const answered = tenant.data.invitations.filter((invitation) => invitation.status !== "pending");
  const recent = tenant.data.invitations.filter((invitation) => invitation.status === "pending" || answered.indexOf(invitation) < 10);
  const earlier = answered.slice(10);
  // A later invitation to the same address replaces the offer to invite again.
  const superseded = (invitation: Invitation) => tenant.data.invitations.some((other) => other.email.toLowerCase() === invitation.email.toLowerCase() && ["pending", "accepted"].includes(other.status));
  const outcome = result || change.error ? (
    <div role={change.error ? "alert" : "status"} className={ui.cx(`${ui.cardRow} grid justify-stretch text-[13px]`)}>
      {change.error ? <p className="text-danger">{change.error.message}</p> : null}
      {result ? <p>{result.message} {deliveryText[result.delivery ?? ""] ?? ""}</p> : null}
      {result?.link ? (
        <div className="flex flex-wrap gap-2">
          <input ref={linkInput} className={ui.cx(`${ui.input} min-h-9 flex-1 font-mono text-xs`)} readOnly value={result.link} aria-label="Invitation link" onFocus={(event) => { event.currentTarget.select(); }} />
          <button className={ui.button()} onClick={() => { void copyLink(result.link ?? ""); }}>{copied ? "Copied" : "Copy link"}</button>
        </div>
      ) : null}
      {result?.link ? <p className={ui.hint}>Anyone with this link can set up the invited account. Share it only with the invited person.</p> : null}
      {copyError ? <p role="alert" className="text-danger">{copyError}</p> : null}
    </div>
  ) : null;
  const inviteForm = (footer: ReactNode, defaults?: FirstInvite) => (
    <form aria-label="Invite a person" {...live.form} onSubmit={(event) => {
      event.preventDefault();
      const form = event.currentTarget;
      const data = new FormData(form);
      change.mutate({ action: "invite", email: formText(data, "email").trim(), name: formText(data, "name").trim(), role: formText(data, "role") }, {
        onSuccess: () => { if (!defaults) form.reset(); },
      });
    }}>
      <div className="grid gap-3 border-t border-outline px-6 py-5 sm:grid-cols-[2fr_2fr_1fr] max-sm:px-[18px]">
        {defaults ? null : <h3 className="text-sm font-semibold sm:col-span-3">Invite a person</h3>}
        <div className="grid content-start gap-1.5">
          <label className={ui.field}>Email address<input className={ui.input} type="email" name="email" required maxLength={254} autoComplete="off" defaultValue={defaults?.email} disabled={change.isPending} /></label>
          <FieldError className="" message={live.errors.email} />
        </div>
        <div className="grid content-start gap-1.5">
          <label className={ui.field}>Full name<input className={ui.input} name="name" required maxLength={160} autoComplete="off" defaultValue={defaults?.name} disabled={change.isPending} /></label>
          <FieldError className="" message={live.errors.name} />
        </div>
        <label className={ui.cx(`${ui.field} content-start`)}>Role<select className={ui.input} name="role" defaultValue={needsOwner ? "owner" : "admin"} disabled={change.isPending}>
          {canSetRoles ? <option value="owner">Owner</option> : null}
          <option value="admin">Administrator</option>
          <option value="member">Member</option>
        </select></label>
      </div>
      <div className={ui.cardFooter}>{footer}</div>
    </form>
  );
  if (welcome && tenant.data.canManage) {
    const sent = change.isSuccess;
    const steps = ["Customer created", "Invite the first person", "Done"];
    return (
      <section className={ui.cx(`${ui.card} mb-8 border-accent/40`)} aria-label="Invite the first person">
        <div className={ui.cardBody}>
          <ol aria-label="Customer setup" className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs">
            {steps.map((label, index) => {
              const state = index === 0 || (index === 1 && sent) ? "done" : index === (sent ? 2 : 1) ? "current" : "pending";
              return (
                <li key={label} data-state={state} className={`flex items-center gap-1.5 ${state === "pending" ? "text-muted" : "text-foreground"}`}>
                  {index > 0 ? <span aria-hidden className={`h-px w-5 ${state === "pending" ? "bg-outline-strong" : "bg-success"}`} /> : null}
                  <span aria-hidden className={`grid size-5 place-items-center rounded-full text-[11px] font-semibold ${{ done: "bg-success text-white", current: "bg-accent text-on-accent", pending: "border border-outline-strong" }[state]}`}>{state === "done" ? "✓" : index + 1}</span>
                  {label}
                </li>
              );
            })}
          </ol>
          <h2 className={ui.sectionTitle}>{sent ? "Invitation sent" : "Invite the first person"}</h2>
          <p className="-mt-2 text-sm text-muted">
            {sent ? "They get one email to set up their account and join. You can follow their progress on this page." : "Usually the contact you just entered, invited as owner. Owners manage the workspace and confirm the customer details after joining."}
          </p>
        </div>
        {outcome}
        {sent ? (
          <div className={ui.cardFooter}>
            <span>Invite more people any time under Members.</span>
            <button className={ui.button("primary")} onClick={welcome.done}>Done</button>
          </div>
        ) : inviteForm(<>
          <button type="button" className={ui.button("quiet")} onClick={welcome.done}>Skip for now</button>
          <button className={ui.button("primary")} disabled={change.isPending}>{change.isPending ? "Sending…" : "Send invitation"}</button>
        </>, welcome)}
      </section>
    );
  }
  const row = (invitation: Invitation) => {
    const [label, tone] = statusOf(invitation);
    return (
      <li key={invitation.id} className={ui.cardRow}>
        <span className="grid min-w-0"><span className="truncate">{invitation.email}</span><span className="text-[13px] text-muted">Invited as {invitation.role}</span></span>
        <span className="flex flex-wrap items-center gap-2">
          <span className={ui.badge(tone)}>{label}</span>
          {invitation.status === "pending" ? <>
            <button className={ui.button()} disabled={busy} onClick={() => { change.mutate({ action: "resend", invitation: invitation.id }); }}>Resend</button>
            <button className={ui.button("quiet")} disabled={busy} onClick={() => { change.mutate({ action: "cancel", invitation: invitation.id }); }}>Cancel invitation</button>
          </> : (invitation.status === "canceled" || invitation.status === "rejected") && !superseded(invitation) ? (
            <button className={ui.button()} disabled={busy} onClick={() => { change.mutate({ action: "invite", email: invitation.email, role: invitation.role, name: invitation.name ?? invitation.email.split("@")[0] }); }}>Invite again</button>
          ) : null}
        </span>
        <Steps invitation={invitation} />
      </li>
    );
  };
  return (
    <>
    <section className={ui.cx(`${ui.card} mt-8`)} aria-label="Members">
      <div className={ui.cardBody}>
        <h2 className={ui.sectionTitle}>Members</h2>
        <p className="-mt-2 text-sm text-muted">Control who can access this workspace. Owners can also change roles and transfer ownership.</p>
      </div>
      {tenant.data.members.length ? (
        <ul>
          {tenant.data.members.map((member) => (
            <li key={member.id} className={ui.cardRow}>
              <span className="grid min-w-0"><strong className="font-medium [overflow-wrap:anywhere]">{member.name}</strong><span className="truncate text-[13px] text-muted" title={member.email}>{member.email}</span></span>
              <span className="flex flex-wrap items-center gap-2">
                {member.mustChangePassword ? <span className={ui.badge("warning")}>Setup pending</span> : null}
                {canSetRoles ? (
                  <select className={ui.cx(ui.input, "min-h-9 w-auto")} aria-label={`Role of ${member.name}`} value={member.role} disabled={change.isPending}
                    onChange={(event) => { change.mutate({ action: "role", member: member.id, role: event.target.value }); }}>
                    <option value="owner">Owner</option>
                    <option value="admin">Administrator</option>
                    <option value="member">Member</option>
                  </select>
                ) : <span className="text-[13px] text-muted capitalize">{member.role}</span>}
                {canSetRoles || (tenant.data.canManage && member.role === "member") ? (
                  <button className={ui.button("quiet")} disabled={change.isPending} onClick={() => { if (change.isError) change.reset(); setRemoving(member); }}>Remove</button>
                ) : null}
              </span>
            </li>
          ))}
        </ul>
      ) : <div className={ui.cx(`${ui.cardRow} text-muted`)}>No one has access yet. Invite the first person below.</div>}
      {outcome}
      {tenant.data.canManage ? inviteForm(<>
        <span>Administrators can invite others and change customer details.</span>
        <button className={ui.button("primary")} disabled={change.isPending}>{change.isPending ? "Sending…" : "Send invitation"}</button>
      </>) : null}
    </section>
    {tenant.data.invitations.length ? (
      <section className={ui.cx(`${ui.card} mt-6`)} aria-label="Invitations">
        <div className={ui.cardBody}>
          <h2 className={ui.sectionTitle}>Invitations</h2>
          <p className="-mt-2 text-sm text-muted">Where each invited person is: invited, email delivered, account set up, joined.</p>
        </div>
        <ul aria-label="Invitation list">{recent.map(row)}</ul>
        {earlier.length ? (
          <details className="border-t border-outline">
            <summary className="cursor-pointer px-6 py-3 text-[13px] text-muted hover:text-foreground max-sm:px-[18px]">Show earlier invitations ({earlier.length})</summary>
            <ul>{earlier.map(row)}</ul>
          </details>
        ) : null}
      </section>
    ) : null}
    {tenant.data.activity.length ? (
      <section className={ui.cx(`${ui.card} mt-6`)} aria-label="Activity">
        <div className={ui.cardBody}>
          <h2 className={ui.sectionTitle}>Activity</h2>
        </div>
        <ul>
          {tenant.data.activity.map((event) => (
            <li key={event.id} className={ui.cx(`${ui.cardRow} text-[13px]`)}>
              <span className="flex min-w-0 flex-wrap items-center gap-x-2">
                <span className="font-medium">{actionText[event.action] ?? event.action}</span>
                {event.outcome === "failed" ? <span className={ui.badge("danger")}>Failed</span> : null}
                <span className="truncate text-muted">{event.actorName ?? event.actorEmail ?? "Unknown person"}</span>
              </span>
              <time dateTime={event.createdAt} title={new Date(event.createdAt).toLocaleString()} className="text-muted">{ago(event.createdAt)}</time>
            </li>
          ))}
        </ul>
      </section>
    ) : null}
    {removing ? (
      <Modal title={`Remove ${removing.name}?`} close={() => { setRemoving(undefined); }} dismissible={!change.isPending}>
        <p className="text-sm [overflow-wrap:anywhere]">{removing.email} will lose access to this workspace. Their account and access to other workspaces stay intact. You can invite them again later.</p>
        {change.isError && change.variables.action === "remove" ? <p role="alert" className="mt-3 text-sm text-danger">{change.error.message}</p> : null}
        <div className="mt-5 flex flex-wrap justify-end gap-2">
          <button className={ui.button()} disabled={change.isPending} onClick={() => { setRemoving(undefined); }}>Cancel</button>
          <button className={ui.button("danger")} disabled={change.isPending} onClick={() => {
            change.mutate({ action: "remove", member: removing.id }, { onSuccess: () => { setRemoving(undefined); } });
          }}>{change.isPending ? "Removing…" : "Remove member"}</button>
        </div>
      </Modal>
    ) : null}
    </>
  );
}
