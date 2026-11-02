import {getStudioSession} from "@/lib/studio-client";

import { getRequestI18n } from '@webdock/i18n/next';
import { headers } from "next/headers";
import Link from "next/link";
import { redirect } from "next/navigation";
import { AccessError, requireAccessOperator } from "@/lib/access-management";
import { getPlatformSettings, getMailConnectionStatus } from "@/lib/platform";
import { AdminForms } from "./form";

export async function generateMetadata(){ const i18n = await getRequestI18n(); return { title: i18n.t("Platform settings") }; }

export default async function Administration() {
  const i18n = await getRequestI18n();

  try {
    await requireAccessOperator(await headers());
  } catch (error) {
    if (error instanceof AccessError) redirect("/tenants");
    throw error;
  }
  const [settings, connection] = await Promise.all([
    getPlatformSettings(),
    ["enabled", "draining"].includes((await getStudioSession())?.installation?.modules.mail ?? "") ? getMailConnectionStatus() : Promise.resolve(null),
  ]);
  return (
    <div className="access-page">
      <header className="account-heading">
        <div>
          <h1>{i18n.t("Platform settings")}</h1>
          <p className="muted">{i18n.t("Branding, customer access and Mail configuration.")}</p>
        </div>
        <Link
          className="button secondary"
          href={
            new URL(
              "/account",
              process.env.WEBDOCK_AUTH_ISSUER ||
                "https://auth.webdock.dev/api/auth",
            ).href
          }
        >{i18n.t("Account & security")}</Link>
      </header>
      <p><Link className="button secondary" href="/admin/registry-users">{i18n.t("Studio accounts")}</Link></p>
      <AdminForms settings={settings} connection={connection} />
      <p className="muted">{i18n.t("Only platform operators can change these settings. Tenant administrators manage their own tenant. Account registration remains invitation-only.")}</p>
    </div>
  );
}
