import { getRequestI18n } from "@webdock/i18n/next";
import Link from "next/link";
import { AuthPanel } from "@/components/auth-forms";

export default async function Home() {
 const { t } = await getRequestI18n();
  return (
    <AuthPanel title={t("Your Webdock account")}>
      <p className="muted">
        {t("Manage your sign-in and account security in one place.")}</p>
      <Link className="button" href="/account">
        {t("Manage account")}</Link>
    </AuthPanel>
  );
}
