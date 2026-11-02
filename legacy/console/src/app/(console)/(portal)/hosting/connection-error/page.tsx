import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
export default async function ConnectionError() {
  const { t } = await getRequestI18n();
  return (
    <section className="panel">
      <h1>{t("Vercel connection needs attention")}</h1>
      <p role="alert">
        {t(
          "The connection could not be completed. Start a new connection from your hosting page. If it fails again, ask your administrator to check the Vercel integration configuration and permissions.",
        )}
      </p>
      <Link className="button" href="/tenants">
        {t("Return to your customers")}
      </Link>
    </section>
  );
}
