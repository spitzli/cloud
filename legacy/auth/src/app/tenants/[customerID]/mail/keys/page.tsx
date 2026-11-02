import { authLabel } from "@/lib/i18n-labels";
import { getRequestI18n } from "@webdock/i18n/next";
import { headers } from "next/headers";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";
import { auth } from "@/lib/auth";
import { AccessError } from "@/lib/access-management";
import { getTenantMailKeys, MailKeysError } from "@/lib/mail-keys";
import { KeyForm } from "./form";
export async function generateMetadata() { const { t } = await getRequestI18n(); return { title: t("Mail credentials") }; }
export default async function MailKeys({ params }: { params: Promise<{ customerID: string }> }) {
 const { t, error: translateError } = await getRequestI18n();
 const { customerID } = await params, requestHeaders = await headers();
 if (!(await auth.api.getSession({ headers: requestHeaders }))) redirect("/sign-in");
 let data;
 try { data = await getTenantMailKeys(requestHeaders, customerID); }
 catch (error) { if (error instanceof AccessError) notFound(); if (error instanceof MailKeysError) return <section className="auth-panel"><h1>{t("Mail credentials")}</h1><p className="notice error" role="alert">{translateError(error.message)}</p><Link href={`/tenants/${customerID}/mail`}>{t("Back to Mail")}</Link></section>; throw error; }
 return <div className="access-page"><header className="account-heading"><div><h1>{t("Mail credentials")}</h1><p className="muted">{data.tenant.name}</p></div><Link className="button secondary" href={`/tenants/${customerID}/mail`}>{t("Back to Mail")}</Link></header>
  <p>{t("Manage this tenant’s SMTP and API credentials directly in Webdock. Secrets are shown once when created and cannot be retrieved later.")}</p>
  <section className="auth-panel"><h2>{t("Connection details")}</h2><dl><dt>{t("SMTP server")}</dt><dd><code>{data.smtpHost}</code></dd><dt>{t("Port and encryption")}</dt><dd>{t("587 with STARTTLS, or 465 with TLS")}</dd><dt>{t("SMTP username")}</dt><dd>{t("Your credential’s Consumer key")}</dd><dt>{t("SMTP password")}</dt><dd>{t("The Consumer secret shown when you create that credential")}</dd><dt>{t("Sending API")}</dt><dd style={{ overflowWrap: "anywhere" }}><code>{data.sendAPI}</code></dd></dl><p>{t("Select SMTP sending or API sending when creating the credential for your application.")}</p></section>
  {(!data.enabled || !data.active) && <p className="notice">{!data.enabled ? t("Mail is disabled.") : t("Sending is paused.")} {t(" You can still view and revoke existing credentials.")}</p>}
  {data.enabled && data.active && <section className="auth-panel"><h2>{t("Create credential")}</h2><KeyForm customer={customerID} operator={data.operator} /></section>}
  <section className="account-section"><h2>{t("Existing credentials")}</h2><p>{t("To rotate a credential, create a replacement, update your application, then revoke the old credential.")}</p><div className="access-grid">{data.keys.map(key => <article className="access-record" key={key.consumerKey}><h3>{key.label}</h3><dl><dt>{t("Consumer key")}</dt><dd style={{ overflowWrap: "anywhere" }}><code>{key.consumerKey}</code></dd><dt>{t("Created")}</dt><dd>{key.creation_time}</dd><dt>{t("Permissions")}</dt><dd>{key.permissions.map(value => authLabel(value, t)).join(", ") || t("None")}</dd><dt>{t("Allowed IPs")}</dt><dd style={{ overflowWrap: "anywhere" }}>{key.ips.join(", ") || t("Any IP address")}</dd></dl>{key.is_legacy && <p className="muted">{t("Legacy credential")}</p>}<KeyForm customer={customerID} consumerKey={key.consumerKey} /></article>)}</div>{!data.keys.length && <p>{t("No credentials created yet.")}</p>}</section>
 </div>;
}
