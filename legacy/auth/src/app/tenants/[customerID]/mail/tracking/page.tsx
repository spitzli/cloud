import { getRequestI18n } from "@webdock/i18n/next";
import { headers } from "next/headers";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";
import { auth } from "@/lib/auth";
import { AccessError } from "@/lib/access-management";
import { getTenantTracking } from "@/lib/mail-tracking";
import { TrackingForm } from "./form";
export async function generateMetadata() { const { t } = await getRequestI18n(); return { title: t("Mail tracking") }; }
export default async function Tracking({ params }: { params: Promise<{ customerID: string }> }) {
 const { t, error: translateError, date: formatDate } = await getRequestI18n();
 const { customerID } = await params, requestHeaders = await headers();
 if (!(await auth.api.getSession({ headers: requestHeaders }))) redirect("/sign-in");
 let data; try { data = await getTenantTracking(requestHeaders, customerID); } catch (error) { if (error instanceof AccessError) notFound(); throw error; }
 const editable = data.canManage && !data.unavailable;
 return <div className="access-page">
  <header className="account-heading"><div><h1>{t("Mail tracking")}</h1><p>{data.tenant.name} {t(" · Track opens and clicks with your own domain.")}</p></div><Link className="button secondary" href={`/tenants/${customerID}/mail`}>{t("Back to Mail")}</Link></header>
  {data.unavailable && <p className="notice">{translateError(data.unavailable)} {t(" Contact your Webdock operator to complete setup.")}</p>}
  <p className="muted">{t("Values show the last successful provider check. No email is sent from this screen.")}</p>
  {data.checkedAt && <p>{t("Last checked: ")}{formatDate(data.checkedAt, { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" })} {t(" UTC")}</p>}
  {editable && <TrackingForm customer={customerID} action="refresh" label={t("Refresh tracking status")} />}
  <section className="account-section"><h2>{t("Tracking settings")}</h2><div className="access-grid">
   {([
    ["opening", "Open tracking", data.settings?.opening.enabled, data.settings?.opening.forced],
    ["click", "Click tracking", data.settings?.click.enabled, data.settings?.click.forced],
    ["custom", "Custom tracking domains", data.settings?.custom, false],
    ["matchSender", "Match sender domain", data.settings?.info.match_sender, false],
   ] as const).map(([setting, label, enabled, forced]) => <article className="access-record" key={setting}><h3>{t(label)}</h3><p>{enabled == null ? t("Status unavailable — refresh to check") : enabled ? t("Enabled") : t("Disabled")}{forced ? t(" · Required by account policy") : ""}</p>
    {editable && !forced && <TrackingForm customer={customerID} action="setting" label={t("Update {value1}", {value1: t(label)})}><input type="hidden" name="setting" value={setting} /><label className="field">{t("Choose state")}<select name="value" defaultValue="" required><option value="" disabled>{t("Select a state")}</option><option value="yes">{t("Enabled")}</option><option value="no">{t("Disabled")}</option></select></label></TrackingForm>}
   </article>)}
  </div>{data.settings && <p className="muted">{data.settings.info.all_domains_disabled ? t("All custom domains are disabled.") : t("At least one custom domain is enabled.")} {data.settings.info.no_default_domain ? t("No default tracking domain is selected.") : t("A default tracking domain is selected.")}</p>}</section>
  <section className="account-section"><h2>{t("Custom tracking domains")}</h2><p>{t("Use a subdomain of a verified sender domain. Creating it does not enable open or click tracking.")}</p>
   {editable && (data.senders.length ? <TrackingForm customer={customerID} action="create" label={t("Create tracking domain")}><label className="field">{t("Sender domain")}<select name="senderDomainID" required>{data.senders.map(sender => <option key={sender.id} value={sender.id}>{sender.domain}</option>)}</select></label><label className="field">{t("Subdomain prefix")}<input name="prefix" defaultValue="links" required maxLength={63} pattern="[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?" /></label><p className="muted">{t("For example, links with example.com creates links.example.com.")}</p></TrackingForm> : <p>{t("Add and verify a sender domain in Mail before creating a tracking domain.")}</p>)}
   <div className="access-grid">{data.domains.map(domain => <article className="access-record" key={domain.id}><h3>{domain.domain}</h3>
    {!domain.mapped ? <><p className="notice">{t("Creation needs reconciliation. Refresh to find the reserved domain; do not create another copy.")}</p>{editable && (domain.cancellable ? <TrackingForm customer={customerID} action="cancel-reservation" label={t("Cancel unused reservation")} confirm={t("Cancel this reservation only if the provider confirms no matching tracking domain exists.")}><input type="hidden" name="domainID" value={domain.id} /></TrackingForm> : <p className="muted">{t("An unused reservation can be cancelled after 15 minutes, once a fresh provider check confirms that no domain was created.")}</p>)}</> : <>
     <p>{domain.snapshot.verified === undefined ? t("Status not checked") : domain.snapshot.verified ? t("Verified") : t("DNS verification pending")} · {domain.snapshot.ssl === undefined ? t("HTTPS not checked") : domain.snapshot.ssl ? t("HTTPS ready") : t("HTTPS pending")}</p>
     <p>{domain.snapshot.enabled === undefined ? t("Sending status not checked") : domain.snapshot.enabled ? t("Enabled") : t("Disabled")} · {domain.snapshot.default === undefined ? t("Domain mode not checked") : domain.snapshot.default ? t("Default domain") : t("Dedicated domain")}</p>
     {domain.snapshot.domain_name && domain.snapshot.verification_domain && <><p>{t("Add both CNAME records at your DNS host, then verify. Your DNS host may append the parent domain automatically.")}</p><dl>{[domain.snapshot.domain_name, domain.snapshot.verification_domain].map(name => <div key={name}><dt>{t("CNAME name")}</dt><dd style={{ overflowWrap: "anywhere" }}><code>{name}</code></dd><dt>{t("CNAME target")}</dt><dd><code>{"smtptrack.com"}</code></dd></div>)}</dl></>}
     {editable && <>
      <TrackingForm customer={customerID} action="verify" label={t("Verify tracking domain")}><input type="hidden" name="domainID" value={domain.id} /></TrackingForm>
      {([ ["enabled", "Domain enabled", domain.snapshot.enabled], ["dedicated", "Dedicated to this sender domain", domain.snapshot.default === undefined ? undefined : !domain.snapshot.default] ] as const).map(([action, label, enabled]) => <TrackingForm key={action} customer={customerID} action={action} label={t("Update {value1}", {value1: t(label)})}><input type="hidden" name="domainID" value={domain.id} /><label className="field">{t(label)}<select name="value" required defaultValue={enabled === undefined ? "" : enabled ? "yes" : "no"}><option value="" disabled>{t("Select a state")}</option><option value="yes">{t("Yes")}</option><option value="no">{t("No")}</option></select></label></TrackingForm>)}
      <TrackingForm customer={customerID} action="remove" label={t("Remove tracking domain")} confirm={t("Remove this tracking domain from the sending account. Existing tracked links may stop working.")}><input type="hidden" name="domainID" value={domain.id} /></TrackingForm>
     </>}
    </>}
   </article>)}</div>{!data.domains.length && <p>{t("No custom tracking domains yet.")}</p>}
  </section>
 </div>;
}
