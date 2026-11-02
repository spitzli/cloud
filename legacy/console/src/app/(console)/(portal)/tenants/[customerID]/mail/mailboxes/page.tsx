import Link from "next/link";
import { headers } from "next/headers";
import { getRequestI18n } from "@webdock/i18n/next";
import { getStudioSession } from "@/lib/studio-client";
import { getTenantMailboxes } from "@/lib/mailboxes";
import { MailServiceError } from "@/lib/studio-errors";
import { MailboxForm } from "./forms";
export async function generateMetadata() { const { t } = await getRequestI18n(); return { title: t("Mailboxes") }; }
export default async function Mailboxes({ params }: { params: Promise<{ customerID: string }> }) {
  const { customerID } = await params, i18n = await getRequestI18n();
  const session = await getStudioSession();
  if (session?.preview) return <section className="auth-panel"><h1>{i18n.t("Mailboxes")}</h1><p>{i18n.t("Mailbox access is unavailable in customer preview.")}</p></section>;
  let data;
  try { data = await getTenantMailboxes(await headers(), customerID); }
  catch (error) { if (!(error instanceof MailServiceError)) throw error; return <section className="auth-panel"><h1>{i18n.t("Mailboxes")}</h1><p role="alert">{i18n.error(error.message)}</p></section>; }
  if (!data) return <section className="auth-panel"><h1>{i18n.t("Mailboxes")}</h1><p>{i18n.t("Mailbox management is not available yet.")}</p></section>;
  const base = `/tenants/${customerID}/mail`;
  return <div className="tenant-page">
    <header className="page-heading"><div><Link href={base}>{i18n.t("Mail & Domains")}</Link><h1>{i18n.t("Mailboxes")}</h1><p className="muted">{i18n.t("Mail addresses and explicit access for your team.")}</p></div></header>
    {data.configuration && data.configuration.state !== "succeeded" && <section className="auth-panel">
      <p role="status">{data.configuration.state === "needs_review" ? i18n.t("Operator review required") : i18n.t("Mailbox changes are being applied.")}</p>
      {data.operator && data.canConfigure && data.configuration.state === "needs_review" && <MailboxForm customerID={customerID} action="configuration.retry" label={i18n.t("Retry Mail operation")}><input type="hidden" name="revision" value={data.configuration.revision} /></MailboxForm>}
    </section>}
    <section className="auth-panel"><h2>{i18n.t("Your mailboxes")}</h2>
      {!data.mailboxes.length ? <p>{i18n.t("No mailboxes are available for your account.")}</p> : <div className="table-wrap"><table><thead><tr><th>{i18n.t("Mailbox")}</th><th>{i18n.t("Status")}</th><th>{i18n.t("Storage limit")}</th><th>{i18n.t("Access")}</th></tr></thead><tbody>
        {data.mailboxes.map(box => <tr key={box.id}><td><strong>{box.name}</strong><br />{box.address}</td><td>{box.state === "ready" ? i18n.t("Ready") : box.state === "needs_review" ? i18n.t("Operator review required") : i18n.t("Applying change")}</td><td>{Number(box.quotaBytes) / 1048576} MiB</td><td>{box.canOpen ? <Link href={`${base}/inbox?mailbox=${box.id}`}>{i18n.t("Open mailbox")}</Link> : i18n.t("No mailbox access assigned")}</td></tr>)}
      </tbody></table></div>}
    </section>
    {data.canConfigure && <>
      <section className="auth-panel"><h2>{i18n.t("Enable a mailbox domain")}</h2>
        {!data.verifiedDomains.length ? <p>{i18n.t("Verify domain ownership under Mail & Domains first.")}</p> : <MailboxForm customerID={customerID} action="domain.attach" label={i18n.t("Enable domain")}><label>{i18n.t("Domain")}<select name="senderDomainID" required>{data.verifiedDomains.map(domain => <option key={domain.id} value={domain.id}>{domain.domain}</option>)}</select></label></MailboxForm>}
      </section>
      {!!data.domains.length && <section className="auth-panel"><h2>{i18n.t("Create mailbox")}</h2><MailboxForm customerID={customerID} action="mailbox.create" label={i18n.t("Create mailbox")}>
        <label>{i18n.t("Name")}<input name="name" required maxLength={80} /></label>
        <label>{i18n.t("Address before @")}<input name="localPart" required maxLength={64} autoCapitalize="none" /></label>
        <label>{i18n.t("Domain")}<select name="domainID" required>{data.domains.map(domain => <option key={domain.id} value={domain.id}>{domain.name}</option>)}</select></label>
        <label>{i18n.t("Storage limit (MiB)")}<input name="quotaMiB" type="number" required min={1} max={8192} defaultValue={1024} /></label>
      </MailboxForm></section>}
      {!!data.mailboxes.length && !!data.people.length && <section className="auth-panel"><h2>{i18n.t("Grant mailbox access")}</h2><p className="muted">{i18n.t("Access allows reading, organizing and sending from this mailbox. Administration alone does not grant access.")}</p><MailboxForm customerID={customerID} action="grant" label={i18n.t("Grant access")}>
        <label>{i18n.t("Mailbox")}<select name="mailboxID" required>{data.mailboxes.map(box => <option key={box.id} value={box.id}>{box.address}</option>)}</select></label>
        <label>{i18n.t("Person")}<select name="subject" required>{data.people.map(person => <option key={person.id} value={person.id}>{person.name} · {person.email}</option>)}</select></label>
      </MailboxForm></section>}
    </>}
    {data.canManage && data.domains.some(domain=>domain.dkimKeys.length>0) && <section className="auth-panel"><h2>{i18n.t("Webdock DKIM")}</h2><p className="muted">{i18n.t("Publish these TXT records to authenticate your domain's outgoing mail. Existing DNS records remain unchanged.")}</p>{data.domains.flatMap(domain=>domain.dkimKeys.map(key=><div className="mailbox-dkim-record" key={key.id}><strong>{domain.name}</strong><code>{key.selector}._domainkey.{domain.name}</code><code>v=DKIM1; k=rsa; p={key.publicKey}</code></div>))}</section>}
    {data.canManage && !!data.grants.filter(grant => grant.enabled).length && <section className="auth-panel"><h2>{i18n.t("Mailbox access")}</h2>{data.grants.filter(grant => grant.enabled).map(grant => <div key={`${grant.mailboxID}:${grant.subject}`}><p>{data.mailboxes.find(box => box.id === grant.mailboxID)?.address} · {data.people.find(person => person.id === grant.subject)?.name ?? i18n.t("Unavailable account")}</p><MailboxForm customerID={customerID} action="revoke" label={i18n.t("Revoke access")}><input type="hidden" name="mailboxID" value={grant.mailboxID} /><input type="hidden" name="subject" value={grant.subject} /></MailboxForm></div>)}</section>}
  </div>;
}
