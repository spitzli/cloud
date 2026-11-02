import { OwnMail } from "@/components/hosting/own-mail";
import { uiLabel } from "@/lib/ui-labels";

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
import { getTenantMailService, MailServiceError } from "@/lib/mail-service";
import { MailServicePanel } from "./service-panel";

export async function generateMetadata() {
  const i18n = await getRequestI18n();
  return { title: i18n.t("Mail") };
}

export default async function TenantMail({
  params,
  searchParams, view = "overview", embedded = false,
}: {
  params: Promise<{ customerID: string }>; embedded?: boolean; view?: "overview" | "domains" | "settings";
  searchParams: Promise<{ mailPage?: string; mailSearch?: string }>;
}) {
  const i18n = await getRequestI18n();

  const { customerID } = await params;
  const requestHeaders = await headers();
  if (!(await auth.api.getSession({ headers: requestHeaders })))
    redirect(
      `/api/sso/login?returnTo=${encodeURIComponent(`/tenants/${customerID}/mail`)}`,
    );
  let data, nativeService;
  try {
    data = await getTenantMail(requestHeaders, customerID);
    nativeService = await getTenantMailService(requestHeaders, customerID);
  } catch (error) {
    if (error instanceof AccessError) notFound();
    if (error instanceof TenantMailError || error instanceof PlatformError || error instanceof MailServiceError)
      return (
        <section className="auth-panel">
          <h1>{i18n.t(view === "domains" ? "Sender domains" : view === "settings" ? "Mail settings" : "Mail")}</h1>
          <p className="notice error" role="alert">
            {i18n.error(error.message)}
          </p>
          <Link href={`/tenants/${customerID}`}>
            {i18n.t("Back to tenant")}
          </Link>
        </section>
      );
    throw error;
  }
  const { tenant, operator, canManage, enabled, configured, account } = data;
  const { domains } = await getTenantSenderDomains(requestHeaders, customerID);
  const editable = enabled && canManage && tenant.status === "active";
  const providerEditable = editable && configured;
  const needsReview =
    account?.state === "provisioning" || account?.state === "needs_review";
  return (
    <div className="access-page">
      {!embedded&&<header className="account-heading">
        <div>
          <h1>{i18n.t(view === "domains" ? "Sender domains" : view === "settings" ? "Mail settings" : "Mail")}</h1>
          <p className="muted">
            {tenant.name} ·{" "}
            {tenant.status === "active"
              ? i18n.t("Sending account and domains")
              : i18n.t("Archived tenant — changes disabled")}
          </p>
        </div>
      </header>}
      {view === "overview" && <>
        <section className="account-section"><h2>{i18n.t("Sending")}</h2><p>{account ? i18n.t(uiLabel(account.state)) : i18n.t("Not configured")}</p><p>{i18n.t("Sending-only does not require incoming Mail activation.")}</p>
          <Link className="button secondary" href={`/tenants/${customerID}/mail/${nativeService?.sendingAvailable ? "sending" : "settings"}`}>{i18n.t("Manage sending")}</Link>
        </section>
        {nativeService && <MailServicePanel data={nativeService} />}
      </>}
      <div>
        {!enabled && (
          <p className="notice" role="status">
            {i18n.t("Mail is disabled for tenants.")}{" "}
            {operator
              ? i18n.t(
                  "Enable Mail in platform settings to manage tenant accounts and domains.",
                )
              : i18n.t("Contact your Webdock operator to enable Mail.")}
          </p>
        )}
        {enabled && !configured && (
          <p className="notice" role="status">
            {i18n.t("The sending connection is not configured yet.")}{" "}
            {operator
              ? i18n.t("Connect the master account in Platform Mail settings.")
              : i18n.t(
                  "Your Webdock operator must complete the Mail connection.",
                )}
          </p>
        )}
        {view === "settings" && <>
        <section className="auth-panel">
          <h2>{i18n.t("Sending account")}</h2>
          <p className="muted">
            {i18n.t("Region: ")}
            {data.region === "eu" ? i18n.t("Europe") : i18n.t("Global")}
          </p>
          {account ? (
            <>
              <dl>
                <dt>{i18n.t("Account email")}</dt>
                <dd>{account.email}</dd>
                <dt>{i18n.t("Setup")}</dt>
                <dd>
                  {needsReview
                    ? i18n.t("Operator review required")
                    : i18n.t(uiLabel(account.state))}
                </dd>
                <dt>{i18n.t("Sending status")}</dt>
                <dd>
                  {account.active === undefined
                    ? i18n.t("Not checked")
                    : account.active
                      ? i18n.t("Enabled")
                      : i18n.t("Disabled")}
                </dd>
                <dt>{i18n.t("Email limit")}</dt>
                <dd>{account.limit ?? i18n.t("Not checked")}</dd>
                <dt>{i18n.t("Emails sent")}</dt>
                <dd>{account.sent ?? i18n.t("Not checked")}</dd>
                {account.interval && (
                  <>
                    <dt>{i18n.t("Usage interval")}</dt>
                    <dd>{account.interval}</dd>
                  </>
                )}
                {account.checkedAt && (
                  <>
                    <dt>{i18n.t("Last checked")}</dt>
                    <dd>
                      {new Date(account.checkedAt).toLocaleString(
                        i18n.locale === "de" ? "de-DE" : "en-GB",
                        {
                          timeZone: "UTC",
                        },
                      )}{" "}
                      {i18n.t("UTC")}
                    </dd>
                  </>
                )}
                {operator && account.providerID && (
                  <>
                    <dt>{i18n.t("Provider account ID")}</dt>
                    <dd>{account.providerID}</dd>
                  </>
                )}
              </dl>
              {account.issue && (
                <p className="notice" role="status">
                  {i18n.error(account.issue)}
                </p>
              )}
              {account.state === "provisioning" && (
                <p className="notice">
                  {i18n.t(
                    "Account creation is in progress. If this state persists, ask an operator to investigate before making further changes. Do not create a second account.",
                  )}
                </p>
              )}
              {account.state === "needs_review" && (
                <p className="notice">
                  {i18n.t(
                    "Account creation needs reconciliation. An operator must review the provider dashboard and link the existing account. Do not create a second account.",
                  )}
                </p>
              )}
              {providerEditable && account.providerID && !needsReview && (
                <MailForm
                  customer={customerID}
                  action="refresh"
                  label={i18n.t("Refresh account status")}
                />
              )}
              {providerEditable &&
                operator &&
                account.providerID &&
                !needsReview && (
                  <details>
                    <summary>{i18n.t("Sending controls & quota")}</summary>
                    <div className="access-grid">
                      <MailForm
                        customer={customerID}
                        action="status"
                        label={i18n.t("Update sending status")}
                      >
                        <label className="field">
                          {i18n.t("Sending status")}
                          <select
                            name="active"
                            defaultValue={account.active ? "yes" : "no"}
                          >
                            <option value="yes">{i18n.t("Enabled")}</option>
                            <option value="no">{i18n.t("Disabled")}</option>
                          </select>
                        </label>
                      </MailForm>
                      <MailForm
                        customer={customerID}
                        action="quota"
                        label={i18n.t("Update email limit")}
                      >
                        <label className="field">
                          {i18n.t("Email limit")}
                          <input
                            name="limit"
                            type="number"
                            min={0}
                            max={1000000000}
                            step={1}
                            required
                            defaultValue={account.limit ?? data.defaultLimit}
                          />
                        </label>
                      </MailForm>
                    </div>
                  </details>
                )}
            </>
          ) : (
            <p>{i18n.t("No Mail account has been set up for this tenant.")}</p>
          )}
          {providerEditable && operator && !account && (
            <>
              <h3>{i18n.t("Create sending account")}</h3>
              <p>
                {i18n.t(
                  "Create a dedicated turboSMTP subaccount for this tenant with an initial limit of ",
                )}
                {data.defaultLimit}
                {i18n.t(" emails.")}
              </p>
              <MailForm
                customer={customerID}
                action="provision"
                label={i18n.t("Create sending account")}
              >
                <label className="field">
                  {i18n.t("Account email")}
                  <input
                    name="email"
                    type="email"
                    required
                    maxLength={254}
                    autoComplete="email"
                  />
                </label>
                <label className="field">
                  {i18n.t("First name")}
                  <input
                    name="firstName"
                    required
                    maxLength={50}
                    autoComplete="given-name"
                  />
                </label>
                <label className="field">
                  {i18n.t("Last name")}
                  <input
                    name="lastName"
                    required
                    maxLength={50}
                    autoComplete="family-name"
                  />
                </label>
                <label className="check">
                  <input
                    name="policyAgree"
                    type="checkbox"
                    value="yes"
                    required
                  />
                  <span>
                    {i18n.t(
                      "I am authorized to create this provider subaccount and accept its",
                    )}{" "}
                    <a
                      href="https://turbosmtp.com/terms-and-conditions/"
                      target="_blank"
                      rel="noreferrer"
                    >
                      {i18n.t("terms")}
                    </a>{" "}
                    {i18n.t("for this setup.")}
                  </span>
                </label>
              </MailForm>
            </>
          )}
          {providerEditable &&
            operator &&
            (!account || account.state === "needs_review") && (
              <details open={account?.state === "needs_review"}>
                <summary>{i18n.t("Link an existing sending account")}</summary>
                <p>
                  {i18n.t(
                    "Use the account ID and email from the turboSMTP dashboard. The account will be checked before it is linked to this tenant. Its sending limit will be set to the platform default of",
                  )}{" "}
                  {data.defaultLimit}
                  {i18n.t(" emails.")}
                </p>
                <MailForm
                  customer={customerID}
                  action="link"
                  label={i18n.t("Link existing account")}
                  confirm={i18n.t(
                    "I have checked that this provider account belongs to this tenant.",
                  )}
                >
                  <label className="field">
                    {i18n.t("Provider account ID")}
                    <input name="providerID" required maxLength={160} />
                  </label>
                  <label className="field">
                    {i18n.t("Account email")}
                    <input name="email" type="email" required maxLength={254} />
                  </label>
                </MailForm>
              </details>
            )}
        </section>
        </>}
        {view === "domains" && <>
        <section className="account-section">
          <h2>{i18n.t("Sender domains")}</h2>
          <p>
            {i18n.t(
              "Add your domain, publish the DNS records below and verify ownership. Then register it for sending directly in Webdock.",
            )}
          </p>
          {editable && (
            <MailForm
              customer={customerID}
              action="add-domain"
              label={i18n.t("Add domain")}
            >
              <label className="field">
                {i18n.t("Domain")}
                <input
                  name="domain"
                  required
                  maxLength={253}
                  placeholder="example.com"
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                />
              </label>
            </MailForm>
          )}
          <div className="access-grid">
            {domains.map((domain) => (
              <article className="access-record" key={domain.id}>
                <h3>{domain.domain}</h3>
                <p>
                  {domain.status === "ownership_verified"
                    ? i18n.t("Ownership verified")
                    : i18n.t("Ownership verification pending")}
                </p>
                <p>
                  {i18n.t("Sending:")}{" "}
                  {domain.providerStatus === "verified"
                    ? i18n.t("SPF, DKIM and DMARC verified")
                    : domain.providerStatus === "pending"
                      ? i18n.t("Provider verification pending")
                      : i18n.t("Not registered yet")}
                </p>
                {domain.dnsIssue && (
                  <p className="notice" role="status">
                    {i18n.error(domain.dnsIssue)}
                  </p>
                )}
                {domain.records.map((record) => (
                  <section key={record.record} className="account-section">
                    <h4>
                      {record.record} · {i18n.t(uiLabel(record.status))}
                    </h4>
                    <dl>
                      <dt>{i18n.t("Type")}</dt>
                      <dd>{record.type}</dd>
                      <dt>{i18n.t("Full record name")}</dt>
                      <dd style={{ overflowWrap: "anywhere" }}>
                        <code>{record.name}</code>
                      </dd>
                      <dt>{i18n.t("Value")}</dt>
                      <dd style={{ overflowWrap: "anywhere" }}>
                        <code>{record.value}</code>
                      </dd>
                    </dl>
                    {record.current.length > 0 &&
                      record.status !== "verified" && (
                        <details>
                          <summary>
                            {i18n.t("Currently published records")}
                          </summary>
                          {record.current.map((value, index) => (
                            <p key={index} style={{ overflowWrap: "anywhere" }}>
                              <code>{value}</code>
                            </p>
                          ))}
                        </details>
                      )}
                    {record.status === "conflict" && (
                      <p className="notice">
                        {i18n.t(
                          "Resolve the existing record conflict before changing DNS. Keep the services and policies you already use.",
                        )}
                      </p>
                    )}
                  </section>
                ))}
                <p className="muted">
                  {i18n.t(
                    "Use the full record names above. If your DNS host appends the zone automatically, enter only the part before that zone. Update an existing SPF record instead of adding a second one.",
                  )}
                </p>
                {domain.providerCheckedAt && (
                  <p className="muted">
                    {i18n.t("Sending verification checked:")}{" "}
                    {new Date(domain.providerCheckedAt).toLocaleString(
                      i18n.locale === "de" ? "de-DE" : "en-GB",
                      {
                        timeZone: "UTC",
                      },
                    )}{" "}
                    {i18n.t("UTC")}
                  </p>
                )}
                {domain.busy && (
                  <p role="status">
                    {i18n.t("A domain operation is in progress.")}
                  </p>
                )}
                {editable && !domain.busy && (
                  <>
                    <MailForm
                      customer={customerID}
                      action="verify-domain"
                      label={i18n.t("Check DNS ownership")}
                    >
                      <input type="hidden" name="domainID" value={domain.id} />
                    </MailForm>
                    {providerEditable &&
                      account?.state === "ready" &&
                      domain.status === "ownership_verified" && (
                        <MailForm
                          customer={customerID}
                          action={
                            domain.providerID
                              ? "refresh-sender"
                              : "register-sender"
                          }
                          label={
                            domain.providerID
                              ? i18n.t("Recheck sending DNS")
                              : i18n.t("Register sending domain")
                          }
                        >
                          <input
                            type="hidden"
                            name="domainID"
                            value={domain.id}
                          />
                        </MailForm>
                      )}
                    {providerEditable &&
                      account?.state === "ready" &&
                      domain.providerID && (
                        <MailForm
                          customer={customerID}
                          action="unregister-sender"
                          label={i18n.t("Remove sending registration")}
                          confirm={i18n.t(
                            "Remove this domain from the sending provider. Applications may no longer send from it.",
                          )}
                        >
                          <input
                            type="hidden"
                            name="domainID"
                            value={domain.id}
                          />
                        </MailForm>
                      )}
                    {!domain.providerID && !domain.providerCheckedAt && (
                      <MailForm
                        customer={customerID}
                        action="remove-domain"
                        label={i18n.t("Remove domain")}
                        confirm={i18n.t(
                          "Remove this local domain entry. DNS records remain unchanged.",
                        )}
                      >
                        <input
                          type="hidden"
                          name="domainID"
                          value={domain.id}
                        />
                      </MailForm>
                    )}
                  </>
                )}
              </article>
            ))}
          </div>
          {!domains.length && <p>{i18n.t("No sender domains added yet.")}</p>}
        </section>
        </>}
      </div>
      {view === "settings" && tenant.status === "active" && (
        <OwnMail customerID={customerID} query={await searchParams} />
      )}
    </div>
  );
}
