
import {withAuthenticatedDatabase} from '@/lib/database-context';
import { getRequestI18n } from "@webdock/i18n/next";
import { headers } from "next/headers";
import Link from "next/link";
import { redirect } from "next/navigation";
import { AccessError, requireAccessOperator } from "@/lib/access-management";
import { getPlatformSettings, getMailConnectionStatus } from "@/lib/platform";
import { AdminForms } from "./form";

export async function generateMetadata() { const { t } = await getRequestI18n(); return { title: t("Webdock administration") }; }

export default async function Administration() {

 const { t } = await getRequestI18n();
  try {
    await requireAccessOperator(await headers());
  } catch (error) {
    if (error instanceof AccessError) redirect("/account");
    throw error;
  }
  return withAuthenticatedDatabase(await headers(), async () => {
  const [settings, connection] = await Promise.all([getPlatformSettings(), getMailConnectionStatus()]);
  return <div className="access-page">
    <header className="account-heading">
      <div><p className="eyebrow">{t("Root admin")}</p><h1>{t("Webdock administration")}</h1><p className="muted">{t("Platform identity and Mail configuration for all tenants.")}</p></div>
      <Link className="button secondary" href="/account">{t("My account")}</Link>
    </header>
    <p><Link className="button secondary" href="/admin/plans">{t("Plans and allowances")}</Link></p>
    <AdminForms settings={settings} connection={connection} />
    <p className="muted">{t("Only platform operators can change these settings. Tenant administrators manage their own tenant. Account registration remains invitation-only.")}</p>
  </div>;

});
}
