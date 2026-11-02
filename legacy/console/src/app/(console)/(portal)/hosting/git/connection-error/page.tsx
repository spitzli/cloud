import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
export default async function GitConnectionError() {
  const { t } = await getRequestI18n();
  return <section className="panel">
    <h1>{t("GitHub connection")}</h1>
    <p role="alert">{t("GitHub access could not be verified. Check installation and repository permissions.")}</p>
    <Link className="button" href="/tenants">{t("Return to your customers")}</Link>
  </section>;
}
