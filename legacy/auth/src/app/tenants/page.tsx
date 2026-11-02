import { authLabel } from "@/lib/i18n-labels";
import { getRequestI18n } from "@webdock/i18n/next";
import { headers } from "next/headers";
import Link from "next/link";
import { redirect } from "next/navigation";
import { auth } from "@/lib/auth";
import { listTenants, listTenantInvitations } from "@/lib/tenants";
import { AccessError } from "@/lib/access-management";
export async function generateMetadata() { const { t } = await getRequestI18n(); return { title: t("Your tenants") }; }
export default async function Tenants() {
 const { t } = await getRequestI18n();
  const requestHeaders = await headers();
  if (!(await auth.api.getSession({ headers: requestHeaders }))) redirect("/sign-in");
  let tenants, invitations;
  try { [tenants, invitations] = await Promise.all([listTenants(requestHeaders), listTenantInvitations(requestHeaders)]); }
  catch (error) { if (error instanceof AccessError) redirect("/account"); throw error; }
  return <div className="access-page">
    <header className="account-heading"><div><h1>{t("Your tenants")}</h1><p className="muted">{t("Customer details, people and assigned websites.")}</p></div><Link className="button secondary" href="/account">{t("My account")}</Link></header>
    {invitations.map(invite => <article className="access-record" key={invite.id}><h2>{t("Invitation to ")}{invite.name}</h2><p className="muted">{t("Tenant role: ")}{authLabel(invite.role, t)}</p><Link className="button" href={`/invitation?id=${encodeURIComponent(invite.id)}`}>{t("Review invitation")}</Link></article>)}
    <div className="access-grid">{tenants.map((tenant) => <article className="access-record" key={tenant.id}><h2><Link href={`/tenants/${tenant.id}`}>{tenant.name}</Link></h2><p className="muted">{tenant.status === "active" ? authLabel(tenant.role, t) || t("Platform operator") : t("Archived")}</p></article>)}</div>
    {!tenants.length && <p>{t("No active tenant memberships yet. Accept your emailed invitation to get started.")}</p>}
    <p><Link href="/sites">{t("My websites")}</Link></p>
  </div>;
}
