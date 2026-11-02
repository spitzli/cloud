import { authLabel } from "@/lib/i18n-labels";
import { getRequestI18n } from "@webdock/i18n/next";
import { headers } from "next/headers";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";
import { auth } from "@/lib/auth";
import { AccessError } from "@/lib/access-management";
import { TenantMailError, getTenantMail } from "@/lib/tenant-mail";
import { PlatformError } from "@/lib/platform";
import { MailForm } from "./form";
import { getTenantSenderDomains } from "@/lib/mail-domains";

export async function generateMetadata() { const { t } = await getRequestI18n(); return { title: t("Mail") }; }

export default async function TenantMail({ params }: { params: Promise<{ customerID: string }> }) {
 const { t, error: translateError, date: formatDate, number: formatNumber } = await getRequestI18n();
  const { customerID } = await params;
  const requestHeaders = await headers();
  if (!(await auth.api.getSession({ headers: requestHeaders }))) redirect("/sign-in");
  let data;
  try { data = await getTenantMail(requestHeaders, customerID); }
  catch (error) {
    if (error instanceof AccessError) notFound();
    if (error instanceof TenantMailError || error instanceof PlatformError) return <section className="auth-panel"><h1>{t("Mail")}</h1><p className="notice error" role="alert">{translateError(error.message)}</p><Link href={`/tenants/${customerID}`}>{t("Back to tenant")}</Link></section>;
    throw error;
  }
  const { tenant, operator, canManage, enabled, configured, account } = data;
  const { domains } = await getTenantSenderDomains(requestHeaders, customerID);
  const editable = enabled && canManage && tenant.status === "active";
  const providerEditable = editable && configured;
  const needsReview = account?.state === "provisioning" || account?.state === "needs_review";
  return <div className="access-page">
    <header className="account-heading"><div><h1>{t("Mail")}</h1><p className="muted">{tenant.name} · {tenant.status === "active" ? t("Sending account and domains") : t("Archived tenant — changes disabled")}</p></div><Link className="button secondary" href={`/tenants/${customerID}`}>{t("Back to tenant")}</Link></header>
    {operator && <p><Link href="/admin">{t("Root Mail settings")}</Link></p>}
    {account?.state === "ready" && <p><Link className="button secondary" href={`/tenants/${customerID}/mail/tracking`}>{t("Custom tracking")}</Link></p>}
    {canManage && account?.state === "ready" && <p><Link className="button secondary" href={`/tenants/${customerID}/mail/keys`}>{t("Manage SMTP / API credentials")}</Link></p>}
    {!enabled && <p className="notice" role="status">{t("Mail is disabled for tenants. ")}{operator ? t("Enable Mail in Root settings to manage tenant accounts and domains.") : t("Contact your Webdock operator to enable Mail.")}</p>}
    {enabled && !configured && <p className="notice" role="status">{t("Mail provider credentials are not configured. ")}{operator ? t("Connect the master account in Root Mail settings.") : t("Your Webdock operator must complete the Mail connection.")}</p>}
    <section className="auth-panel"><h2>{t("Sending account")}</h2>
      <p className="muted">{t("Region: ")}{data.region === "eu" ? t("Europe") : t("Global")}</p>
      {account ? <>
        <dl><dt>{t("Account email")}</dt><dd>{account.email}</dd><dt>{t("Setup")}</dt><dd>{needsReview ? t("Operator review required") : authLabel(account.state, t)}</dd>
          <dt>{t("Sending status")}</dt><dd>{account.active === undefined ? t("Not checked") : account.active ? t("Enabled") : t("Disabled")}</dd>
          <dt>{t("Email limit")}</dt><dd>{account.limit == null ? t("Not checked") : formatNumber(account.limit)}</dd><dt>{t("Emails sent")}</dt><dd>{account.sent == null ? t("Not checked") : formatNumber(account.sent)}</dd>
          {account.interval && <><dt>{t("Usage interval")}</dt><dd>{authLabel(account.interval, t)}</dd></>}
          {account.checkedAt && <><dt>{t("Last checked")}</dt><dd>{formatDate(account.checkedAt, { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" })} {t(" UTC")}</dd></>}
          {operator && account.providerID && <><dt>{t("Provider account ID")}</dt><dd>{account.providerID}</dd></>}
        </dl>
        {account.issue && <p className="notice" role="status">{translateError(account.issue)}</p>}
        {account.state === "provisioning" && <p className="notice">{t("Account creation is in progress. If this state persists, ask an operator to investigate before making further changes. Do not create a second account.")}</p>}
        {account.state === "needs_review" && <p className="notice">{t("Account creation needs reconciliation. An operator must review the provider dashboard and link the existing account. Do not create a second account.")}</p>}
        {providerEditable && account.providerID && !needsReview && <MailForm customer={customerID} action="refresh" label={t("Refresh account status")} />}
        {providerEditable && operator && account.providerID && !needsReview && <div className="access-grid">
          <MailForm customer={customerID} action="status" label={t("Update sending status")}><label className="field">{t("Sending status")}<select name="active" defaultValue={account.active ? "yes" : "no"}><option value="yes">{t("Enabled")}</option><option value="no">{t("Disabled")}</option></select></label></MailForm>
          <MailForm customer={customerID} action="quota" label={t("Update email limit")}><label className="field">{t("Email limit")}<input name="limit" type="number" min={0} max={1000000000} step={1} required defaultValue={account.limit ?? data.defaultLimit} /></label></MailForm>
        </div>}
      </> : <p>{t("No Mail account has been set up for this tenant.")}</p>}
      {providerEditable && operator && !account && <>
        <h3>{t("Create sending account")}</h3><p>{t("Create a dedicated turboSMTP subaccount for this tenant with an initial limit of ")}{formatNumber(data.defaultLimit)} {t(" emails.")}</p>
        <MailForm customer={customerID} action="provision" label={t("Create sending account")}>
          <label className="field">{t("Account email")}<input name="email" type="email" required maxLength={254} autoComplete="email" /></label>
          <label className="field">{t("First name")}<input name="firstName" required maxLength={50} autoComplete="given-name" /></label>
          <label className="field">{t("Last name")}<input name="lastName" required maxLength={50} autoComplete="family-name" /></label>
          <label className="check"><input name="policyAgree" type="checkbox" value="yes" required /><span>{t("I am authorized to create this provider subaccount and accept its ")}<a href="https://turbosmtp.com/terms-and-conditions/" target="_blank" rel="noreferrer">{t("terms")}</a> {t(" for this setup.")}</span></label>
        </MailForm>
      </>}
      {providerEditable && operator && (!account || account.state === "needs_review") && <>
        <h3>{t("Link an existing account")}</h3><p>{t("Use the account ID and email from the turboSMTP dashboard. The account will be checked before it is linked to this tenant. Its sending limit will be set to the platform default of ")}{formatNumber(data.defaultLimit)} {t(" emails.")}</p>
        <MailForm customer={customerID} action="link" label={t("Link existing account")} confirm={t("I have checked that this provider account belongs to this tenant.")}>
          <label className="field">{t("Provider account ID")}<input name="providerID" required maxLength={160} /></label>
          <label className="field">{t("Account email")}<input name="email" type="email" required maxLength={254} /></label>
        </MailForm>
      </>}
    </section>
    <section className="account-section"><h2>{t("Sender domains")}</h2>
      <p>{t("Add your domain, publish the DNS records below and verify ownership. Then register it for sending directly in Webdock.")}</p>
      {editable && <MailForm customer={customerID} action="add-domain" label={t("Add domain")}><label className="field">{t("Domain")}<input name="domain" required maxLength={253} placeholder="example.com" autoCapitalize="none" autoCorrect="off" spellCheck={false} /></label></MailForm>}
      <div className="access-grid">{domains.map(domain => <article className="access-record" key={domain.id}>
        <h3>{domain.domain}</h3>
        <p>{domain.status === "ownership_verified" ? t("Ownership verified") : t("Ownership verification pending")}</p>
        <p>{t("Sending: ")}{domain.providerStatus === "verified" ? t("SPF, DKIM and DMARC verified") : domain.providerStatus === "pending" ? t("Provider verification pending") : t("Not registered yet")}</p>
        {domain.dnsIssue && <p className="notice" role="status">{domain.dnsIssue}</p>}
        {domain.records.map(record => <section key={record.record} className="account-section">
          <h4>{record.record} · {authLabel(record.status, t)}</h4>
          <dl><dt>{t("Type")}</dt><dd>{record.type}</dd><dt>{t("Full record name")}</dt><dd style={{ overflowWrap: "anywhere" }}><code>{record.name}</code></dd><dt>{t("Value")}</dt><dd style={{ overflowWrap: "anywhere" }}><code>{record.value}</code></dd></dl>
          {record.current.length > 0 && record.status !== "verified" && <details><summary>{t("Currently published records")}</summary>{record.current.map((value, index) => <p key={index} style={{ overflowWrap: "anywhere" }}><code>{value}</code></p>)}</details>}
          {record.status === "conflict" && <p className="notice">{t("Resolve the existing record conflict before changing DNS. Keep the services and policies you already use.")}</p>}
        </section>)}
        <p className="muted">{t("Use the full record names above. If your DNS host appends the zone automatically, enter only the part before that zone. Update an existing SPF record instead of adding a second one.")}</p>
        {domain.providerCheckedAt && <p className="muted">{t("Sending verification checked: ")}{formatDate(domain.providerCheckedAt, { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" })} {t(" UTC")}</p>}
        {domain.busy && <p role="status">{t("A domain operation is in progress.")}</p>}
        {editable && !domain.busy && <>
          <MailForm customer={customerID} action="verify-domain" label={t("Check DNS ownership")}><input type="hidden" name="domainID" value={domain.id} /></MailForm>
          {providerEditable && account?.state === "ready" && domain.status === "ownership_verified" && <MailForm customer={customerID} action={domain.providerID ? "refresh-sender" : "register-sender"} label={domain.providerID ? t("Recheck sending DNS") : t("Register sending domain")}><input type="hidden" name="domainID" value={domain.id} /></MailForm>}
          {providerEditable && account?.state === "ready" && domain.providerID && <MailForm customer={customerID} action="unregister-sender" label={t("Remove sending registration")} confirm={t("Remove this domain from the sending provider. Applications may no longer send from it.")}><input type="hidden" name="domainID" value={domain.id} /></MailForm>}
          {!domain.providerID && !domain.providerCheckedAt && <MailForm customer={customerID} action="remove-domain" label={t("Remove domain")} confirm={t("Remove this local domain entry. DNS records remain unchanged.")}><input type="hidden" name="domainID" value={domain.id} /></MailForm>}
        </>}
      </article>)}</div>
      {!domains.length && <p>{t("No sender domains added yet.")}</p>}
    </section>
  </div>;
}
