
import { getRequestI18n } from '@webdock/i18n/next';
import { headers } from "next/headers";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";
import { auth } from "@/lib/auth";
import { AccessError } from "@/lib/access-management";
import { getTenantMailKeys, MailKeysError } from "@/lib/mail-keys";
import { KeyForm } from "./form";
export async function generateMetadata(){ const i18n = await getRequestI18n(); return { title: i18n.t("Mail credentials") }; }
export default async function MailKeys({
  params,
}: {
  params: Promise<{ customerID: string }>;
}) {
  const i18n = await getRequestI18n();

  const { customerID } = await params,
    requestHeaders = await headers();
  if (!(await auth.api.getSession({ headers: requestHeaders })))
    redirect(
      `/api/sso/login?returnTo=${encodeURIComponent(`/tenants/${customerID}/mail/keys`)}`,
    );
  let data;
  try {
    data = await getTenantMailKeys(requestHeaders, customerID);
  } catch (error) {
    if (error instanceof AccessError) notFound();
    if (error instanceof MailKeysError && error.message === "Use Webdock Mail sending to manage native API keys.") redirect(`/tenants/${customerID}/mail/sending`);
    if (error instanceof MailKeysError)
      return (
        <section className="auth-panel">
          <h1>{i18n.t("Mail credentials")}</h1>
          <p className="notice error" role="alert">
            {i18n.error(error.message)}
          </p>
          <Link href={`/tenants/${customerID}/mail`}>{i18n.t("Back to Mail")}</Link>
        </section>
      );
    throw error;
  }
  return (
    <div className="access-page">
      <header className="account-heading">
        <div>
          <h1>{i18n.t("Mail credentials")}</h1>
          <p className="muted">{data.tenant.name}</p>
        </div>
      </header>
      <section className="auth-panel">
        <h2>{i18n.t("Connection details")}</h2>
        <dl>
          <dt>{i18n.t("SMTP server")}</dt>
          <dd>
            <code>{data.smtpHost}</code>
          </dd>
          <dt>{i18n.t("Port and encryption")}</dt>
          <dd>{i18n.t("587 with STARTTLS, or 465 with TLS")}</dd>
          <dt>{i18n.t("SMTP username")}</dt>
          <dd>{i18n.t("Your credential’s Consumer key")}</dd>
          <dt>{i18n.t("SMTP password")}</dt>
          <dd>{i18n.t("The Consumer secret shown when you create that credential")}</dd>
          <dt>{i18n.t("Sending API")}</dt>
          <dd style={{ overflowWrap: "anywhere" }}>
            <code>{data.sendAPI}</code>
          </dd>
        </dl>
        <p>{i18n.t("Select SMTP sending or API sending when creating the credential for your application.")}</p>
      </section>
      {(!data.enabled || !data.active) && (
        <p className="notice">
          {!data.enabled ? i18n.t("Mail is disabled.") : i18n.t("Sending is paused.")}{i18n.t(" You can still view and revoke existing credentials.")}</p>
      )}
      {data.enabled && data.active && (
        <section className="auth-panel">
          <h2>{i18n.t("Create credential")}</h2>
          <p>{i18n.t("Manage this tenant’s SMTP and API credentials directly in Webdock. Secrets are shown once when created and cannot be retrieved later.")}</p>
          <KeyForm customer={customerID} operator={data.operator} />
        </section>
      )}
      <section className="account-section">
        <h2>{i18n.t("Existing credentials")}</h2>
        <p>{i18n.t("To rotate a credential, create a replacement, update your application, then revoke the old credential.")}</p>
        <div className="access-grid">
          {data.keys.map((key) => (
            <article className="access-record" key={key.consumerKey}>
              <h3>{key.label}</h3>
              <dl>
                <dt>{i18n.t("Consumer key")}</dt>
                <dd style={{ overflowWrap: "anywhere" }}>
                  <code>{key.consumerKey}</code>
                </dd>
                <dt>{i18n.t("Created")}</dt>
                <dd>{key.creation_time}</dd>
                <dt>{i18n.t("Permissions")}</dt>
                <dd>{key.permissions.join(", ") || i18n.t("None")}</dd>
                <dt>{i18n.t("Allowed IPs")}</dt>
                <dd style={{ overflowWrap: "anywhere" }}>
                  {key.ips.join(", ") || i18n.t("Any IP address")}
                </dd>
              </dl>
              {key.is_legacy && <p className="muted">{i18n.t("Legacy credential")}</p>}
              <KeyForm customer={customerID} consumerKey={key.consumerKey} />
            </article>
          ))}
        </div>
        {!data.keys.length && <p>{i18n.t("No credentials created yet.")}</p>}
      </section>
    </div>
  );
}
