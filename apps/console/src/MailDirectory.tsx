import type { MailDirectory } from "./mail-contracts";
import { Link } from "@tanstack/react-router";
import { useApi, useSession } from "./context";
import * as ui from "./ui";
const empty = `${ui.cardRow} text-muted`;
export function MailDirectoryView({ value }: { value: MailDirectory }) {
  const api = useApi();
  const session = useSession();
  const webmail = Boolean(api.config.mailGatewayOrigin) && !(session.user.preview && !session.user.acting);
  const configuration = value.configuration;
  return (
    <>
      <section className={ui.cx(`${ui.card} ${ui.cardBody}`)}>
        <h2 className={ui.sectionTitle}>Mailbox configuration</h2>
        {configuration ? (
          <>
            <div className="flex flex-wrap items-center gap-3 text-sm">
              <p>
                Requested revision {configuration.revision} · Applied revision{" "}
                {configuration.appliedRevision}
              </p>
              <p className={ui.cx(`${ui.badge(ui.statusTone(configuration.state ?? undefined))} normal-case`)}>
                {configuration.state?.replaceAll("_", " ") ??
                  "No operation recorded"}
              </p>
            </div>
            {configuration.revision !== configuration.appliedRevision ? (
              <p role="status" className={ui.notice}>
                These changes are not yet applied. Queued domains, mailboxes and
                grants do not provide working delivery or access.
              </p>
            ) : null}
          </>
        ) : (
          <p className="text-sm text-muted">No mailbox configuration recorded.</p>
        )}
        <p className="text-[13px] text-muted">
          {webmail ? "Mailbox content requires your current explicit access grant." : "Webmail is unavailable for this session or installation."}
        </p>
      </section>
      <section className={ui.card}>
        <h2 className={ui.cx(`${ui.sectionTitle} p-6 max-sm:p-[18px]`)}>Mailboxes</h2>
        {value.mailboxes.length ? (
          <ul>
            {value.mailboxes.map((mailbox) => (
              <li key={mailbox.id} className={ui.cx(ui.cardRow, webmail && mailbox.canOpen && ui.clickableRow)}>
                <div className="min-w-0">
                  <strong className="font-medium">{mailbox.name}</strong>
                  <p className="text-[13px]">{mailbox.address}</p>
                  <p className="text-[13px] text-muted">
                    {mailbox.state.replaceAll("_", " ")} ·{" "}
                    {mailbox.enabled ? "Enabled" : "Disabled"} · Quota{" "}
                    {mailbox.quotaBytes} bytes
                  </p>
                  {webmail && mailbox.canOpen ? <Link className={ui.cx(`${ui.button()} mt-2`, ui.rowLink)} to="/mail/inbox/$mailboxID" params={{ mailboxID: mailbox.id }} search={{ customer: value.customerID }}>Open mailbox</Link> : null}
                </div>
              </li>
            ))}
          </ul>
        ) : (
          <p className={empty}>No mailboxes are visible to your account.</p>
        )}
      </section>
      <section className={ui.card}>
        <h2 className={ui.cx(`${ui.sectionTitle} p-6 max-sm:p-[18px]`)}>Domains</h2>
        {value.domains.length ? (
          <ul>
            {value.domains.map((domain) => (
              <li key={domain.id} className={ui.cardRow}>
                <strong className="font-medium">{domain.name}</strong>
                <span className={ui.cx(`${ui.badge(ui.statusTone(domain.state))} normal-case`)}>{domain.state.replaceAll("_", " ")}</span>
              </li>
            ))}
          </ul>
        ) : (
          <p className={empty}>No incoming domains are attached.</p>
        )}
      </section>
      {value.canManage ? (
        <section className={ui.card}>
          <h2 className={ui.cx(`${ui.sectionTitle} p-6 max-sm:p-[18px]`)}>Mailbox access</h2>
          {value.grants.length ? (
            <ul>
              {value.grants.map((grant) => (
                <li key={`${grant.mailboxID}:${grant.subject}`} className={ui.cardRow}>
                  <div className="min-w-0">
                    <strong className="font-medium">
                      {value.mailboxes.find(
                        (mailbox) => mailbox.id === grant.mailboxID,
                      )?.address ?? grant.mailboxID}
                    </strong>
                    <p className="text-[13px]">
                      {value.people.find(
                        (person) => person.id === grant.subject,
                      )?.name ?? grant.subject}
                    </p>
                    <p className="text-[13px] text-muted">
                      {grant.enabled ? "Requested access" : "Revoked"} ·
                      Revision {grant.revision}
                    </p>
                  </div>
                </li>
              ))}
            </ul>
          ) : (
            <p className={empty}>
              No mailbox grants recorded. Operator access does not grant mailbox
              content access.
            </p>
          )}
        </section>
      ) : null}
    </>
  );
}
