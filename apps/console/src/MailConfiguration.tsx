import { useState, type ReactNode } from "react";
import { useSession } from "./context";
import { formText } from "./form";
import {
  mailboxCommandSchema,
  type MailDirectory,
  type MailboxCommand,
} from "./mail-contracts";
import { useConfigurationAction } from "./mail-configuration-client";
import * as ui from "./ui";
const check = "flex items-center gap-2 text-[13px] font-medium";
const checkbox = "size-4 shrink-0 accent-accent";
export function MailConfiguration({
  value,
  readAt,
  refresh,
  provisioningBlocked,
}: {
  value: MailDirectory;
  readAt: number;
  refresh: () => Promise<void>;
  provisioningBlocked: boolean;
}) {
  const session = useSession();
  const configure = useConfigurationAction(value, readAt, "configure", refresh);
  const revoke = useConfigurationAction(value, readAt, "revoke", refresh);
  if (!value.canManage) return null;
  const preview = Boolean((session.user.preview && !session.user.acting));
  const disabled =
    preview || !value.canConfigure || provisioningBlocked || configure.blocked;
  return (
    <section className={ui.cx(`${ui.card} ${ui.cardBody}`)}>
      <h2 className={ui.sectionTitle}>Configure incoming mail</h2>
      <p className="-mt-2 text-sm text-muted">
        Configuration requests are queued. Delivery and remote mailbox access
        depend on the recorded applied revision.
      </p>
      <RequestNotice action={configure} />
      <div className="grid items-start gap-6 lg:grid-cols-2">
        <AttachDomain
          value={value}
          disabled={disabled}
          submit={configure.submit}
        />
        <CreateMailbox
          value={value}
          disabled={disabled}
          submit={configure.submit}
        />
        <GrantAccess
          value={value}
          disabled={disabled}
          submit={configure.submit}
        />
        <RevokeAccess
          value={value}
          disabled={preview || revoke.blocked}
          submit={revoke.submit}
        />
      </div>
      <RequestNotice action={revoke} />
      {value.operator && value.canRetryConfiguration && value.configuration ? (
        <div className={ui.cx(`${ui.notice} grid justify-items-start gap-3`)}>
          <p>
            Configuration revision {value.configuration.revision} has unapplied
            changes. A retry records fresh authorization and a new requested
            revision.
          </p>
          <button
            className={ui.button()}
            disabled={disabled}
            onClick={() => {
              if (value.configuration)
                configure.submit({
                  action: "configuration.retry",
                  revision: value.configuration.revision,
                });
            }}
          >
            Retry configuration
          </button>
        </div>
      ) : null}
    </section>
  );
}
interface WriteProps {
  value: MailDirectory;
  disabled: boolean;
  submit: (input: MailboxCommand) => void;
}
function AttachDomain({ value, disabled, submit }: WriteProps) {
  const available = value.verifiedDomains.filter(
    (domain) =>
      !value.domains.some((attached) => attached.name === domain.domain),
  );
  return (
    <ConfigurationForm
      title="Attach incoming domain"
      disabled={disabled || available.length === 0}
      submit={(data) => {
        const senderDomainID = formText(data, "senderDomainID");
        if (!available.some((domain) => domain.id === senderDomainID))
          throw new Error("Choose a currently verified domain.");
        submit({ action: "domain.attach", senderDomainID });
      }}
    >
      <label className={ui.field}>
        Verified sending domain
        <select className={ui.input} name="senderDomainID" required defaultValue="">
          <option value="">Choose a verified domain</option>
          {available.map((domain) => (
            <option key={domain.id} value={domain.id}>
              {domain.domain}
            </option>
          ))}
        </select>
      </label>
      <p className={ui.hint}>
        Only domains whose ownership is verified for this workspace are offered.
        Attaching queues incoming configuration.
      </p>
    </ConfigurationForm>
  );
}
function CreateMailbox({ value, disabled, submit }: WriteProps) {
  const domains = value.domains.filter((domain) => domain.state === "ready");
  return (
    <ConfigurationForm
      title="Create mailbox"
      disabled={disabled || domains.length === 0}
      submit={(data) => {
        const input = mailboxCommandSchema.parse({
          action: "mailbox.create",
          domainID: formText(data, "domainID"),
          localPart: formText(data, "localPart"),
          name: formText(data, "name"),
          quotaBytes: formText(data, "quotaBytes"),
        });
        if (input.action !== "mailbox.create") return;
        const domain = domains.find((domain) => domain.id === input.domainID);
        if (!domain) throw new Error("Choose a ready incoming domain.");
        if (
          value.mailboxes.some(
            (mailbox) =>
              mailbox.address === `${input.localPart}@${domain.name}`,
          )
        )
          throw new Error(
            "This address already has a recorded mailbox. Refresh and review it.",
          );
        submit(input);
      }}
    >
      <label className={ui.field}>
        Incoming domain
        <select className={ui.input} name="domainID" required defaultValue="">
          <option value="">Choose a ready domain</option>
          {domains.map((domain) => (
            <option key={domain.id} value={domain.id}>
              {domain.name}
            </option>
          ))}
        </select>
      </label>
      <label className={ui.field}>
        Local part
        <input
          className={ui.input}
          name="localPart"
          required
          maxLength={64}
          pattern="[a-z0-9]+([._+\-][a-z0-9]+)*"
          placeholder="support"
          title="Use lowercase letters and numbers, optionally separated by dots, underscores, plus signs or hyphens."
        />
      </label>
      <label className={ui.field}>
        Mailbox name
        <input className={ui.input} name="name" required maxLength={80} />
      </label>
      <label className={ui.field}>
        Quota in bytes
        <input
          className={ui.input}
          name="quotaBytes"
          required
          inputMode="numeric"
          pattern="[0-9]+"
          defaultValue="1073741824"
        />
      </label>
      <p className={ui.hint}>
        Quota: 1048576–8589934592 bytes. Total workspace capacity is checked by
        the server.
      </p>
    </ConfigurationForm>
  );
}
function GrantAccess({ value, disabled, submit }: WriteProps) {
  const mailboxes = value.mailboxes.filter((mailbox) => mailbox.enabled);
  return (
    <ConfigurationForm
      title="Grant mailbox access"
      disabled={disabled || mailboxes.length === 0 || value.people.length === 0}
      submit={(data) => {
        const mailboxID = formText(data, "mailboxID");
        const subject = formText(data, "subject");
        if (
          !mailboxes.some((mailbox) => mailbox.id === mailboxID) ||
          !value.people.some((person) => person.id === subject)
        )
          throw new Error("Choose a currently available mailbox and person.");
        if (
          value.grants.some(
            (grant) =>
              grant.mailboxID === mailboxID &&
              grant.subject === subject &&
              grant.enabled,
          )
        )
          throw new Error(
            "Access is already requested for this person. Review its configuration state.",
          );
        if (formText(data, "confirm") !== "yes")
          throw new Error("Confirm this explicit mailbox grant.");
        submit({ action: "grant", mailboxID, subject });
      }}
    >
      <label className={ui.field}>
        Mailbox
        <select className={ui.input} name="mailboxID" required defaultValue="">
          <option value="">Choose a mailbox</option>
          {mailboxes.map((mailbox) => (
            <option key={mailbox.id} value={mailbox.id}>
              {mailbox.address}
            </option>
          ))}
        </select>
      </label>
      <label className={ui.field}>
        Person
        <select className={ui.input} name="subject" required defaultValue="">
          <option value="">Choose an eligible person</option>
          {value.people.map((person) => (
            <option key={person.id} value={person.id}>
              {person.name} ({person.email})
            </option>
          ))}
        </select>
      </label>
      <label className={check}>
        <input className={checkbox} type="checkbox" name="confirm" value="yes" required />
        Confirm access to this mailbox’s content
      </label>
      <p className={ui.hint}>
        Eligible members and operators are supplied by the server. Operator
        status alone never grants access. A recorded grant does not mean remote
        access is applied.
      </p>
    </ConfigurationForm>
  );
}
function RevokeAccess({ value, disabled, submit }: WriteProps) {
  const grants = value.grants.filter((grant) => grant.enabled);
  return (
    <ConfigurationForm
      title="Revoke mailbox access"
      disabled={disabled || grants.length === 0}
      submit={(data) => {
        const selected = formText(data, "grant");
        const grant = grants.find(
          (grant) => `${grant.mailboxID}:${grant.subject}` === selected,
        );
        if (!grant || formText(data, "confirm") !== "yes")
          throw new Error("Choose and confirm the access grant to revoke.");
        submit({
          action: "revoke",
          mailboxID: grant.mailboxID,
          subject: grant.subject,
        });
      }}
    >
      <label className={ui.field}>
        Existing access
        <select className={ui.input} name="grant" required defaultValue="">
          <option value="">Choose an access grant</option>
          {grants.map((grant) => (
            <option
              key={`${grant.mailboxID}:${grant.subject}`}
              value={`${grant.mailboxID}:${grant.subject}`}
            >
              {value.mailboxes.find((mailbox) => mailbox.id === grant.mailboxID)
                ?.address ?? grant.mailboxID}{" "}
              —{" "}
              {value.people.find((person) => person.id === grant.subject)
                ?.name ?? grant.subject}
            </option>
          ))}
        </select>
      </label>
      <label className={check}>
        <input className={checkbox} type="checkbox" name="confirm" value="yes" required />
        Confirm access revocation
      </label>
      <p className={ui.hint}>
        After the server accepts revocation, the Spitzli Cloud gateway denies this
        access immediately. Remote configuration is applied separately.
        Revocation stays available while provisioning is blocked.
      </p>
    </ConfigurationForm>
  );
}
function ConfigurationForm({
  title,
  disabled,
  submit,
  children,
}: {
  title: string;
  disabled: boolean;
  submit: (data: FormData) => void;
  children: ReactNode;
}) {
  const [error, setError] = useState<string | null>(null);
  return (
    <form
      className="grid min-w-0 content-start gap-3 rounded-lg border border-outline p-5 max-sm:p-4"
      aria-label={title}
      onChange={(event) => {
        if (!(event.target instanceof HTMLSelectElement)) return;
        for (const checkbox of event.currentTarget.querySelectorAll<HTMLInputElement>(
          'input[type="checkbox"]',
        ))
          checkbox.checked = false;
      }}
      onSubmit={(event) => {
        event.preventDefault();
        if (disabled) return;
        setError(null);
        try {
          submit(new FormData(event.currentTarget));
          for (const checkbox of event.currentTarget.querySelectorAll<HTMLInputElement>(
            'input[type="checkbox"]',
          ))
            checkbox.checked = false;
        } catch (error) {
          setError(
            error instanceof Error && error.name !== "ZodError"
              ? error.message
              : "Check the entered values and quota bounds.",
          );
        }
      }}
    >
      <fieldset disabled={disabled} className="m-0 grid min-w-0 gap-3 border-0 p-0">
        <legend className="px-0 pb-3 text-sm font-semibold">{title}</legend>
        {children}
        <button className={ui.cx(`${ui.button()} justify-self-start`)} type="submit">
          {title}
        </button>
      </fieldset>
      {error ? (
        <p role="alert" className="text-sm text-danger">
          {error}
        </p>
      ) : null}
    </form>
  );
}
function RequestNotice({
  action,
}: {
  action: ReturnType<typeof useConfigurationAction>;
}) {
  if (!action.state) return null;
  const messages = {
    pending: "Configuration request in progress",
    accepted: "Configuration change recorded",
    uncertain: "Configuration outcome is unknown",
    rejected: "Configuration request rejected",
    denied: "Configuration access changed",
  };
  return (
    <div role="status" className={ui.cx(`${ui.notice} grid justify-items-start gap-2`)}>
      <h3 className="text-sm font-semibold">{messages[action.state]}</h3>
      <p className="text-muted">
        Refresh to verify the current directory. Changes are not resubmitted
        automatically. An unknown creation remains blocked until its exact
        requested resource is recorded.
      </p>
      <button
        className={ui.button()}
        disabled={action.state === "pending"}
        onClick={() => void action.review()}
      >
        Refresh configuration
      </button>
    </div>
  );
}
