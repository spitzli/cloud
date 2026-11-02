import { getRequestI18n } from '@webdock/i18n/next';
import type { Metadata } from 'next';

export async function generateMetadata(): Promise<Metadata> {
  const { t } = await getRequestI18n();
  return {
    title: t("Legal notice — Spitzli Cloud"),
    description: t("Spitzli Cloud provider and contact details for Spitzli Development."),
    alternates: { canonical: '/imprint' },
    openGraph: { title: t("Legal notice — Spitzli Cloud"), url: '/imprint' },
  };
}

export default async function Imprint() {
  const { t, date, locale } = await getRequestI18n();
  return <article>
    <p className="subtle-label">{t("Spitzli Cloud · Updated {date}", { date: date("2026-10-14T12:00:00Z", { dateStyle: "long" }) })}</p>
    <h1>{t("Legal notice")}</h1>
    <p>{t("Provider of Spitzli Cloud pursuant to Section 5 DDG:")}</p>
    <p>Spitzli Development<br/>{t("Owner: Dominik Spitzli · Sole trader")}<br/>Emsoldstrasse 40<br/>26180 Rastede<br/>{t("Germany")}</p>
    <h2>{t("Contact")}</h2>
    <p>{t("Phone")}: <a href="tel:+4915679657952">+49 1567 9657952</a></p>
    <p><a href="mailto:dominik@spitzli.dev">dominik@spitzli.dev</a><br/><a href="mailto:info@spitzli.dev">info@spitzli.dev</a></p>
    <h2>{t("VAT identification number")}</h2>
    <p>{t("VAT identification number pursuant to Section 27a of the German VAT Act: DE463646700.")}</p>
    <h2>{t("Spitzli Cloud and Spitzli Development")}</h2>
    <p>{t("Spitzli Cloud is a service of Spitzli Development. These provider details apply to Spitzli Cloud’s own website and services. Independently operated customer websites are the responsibility of their respective providers.")}</p>
    <p><a href={locale === "de" ? "https://spitzli.dev/de/impressum" : "https://spitzli.dev/en/legal-notice"}>{t("Spitzli Development legal notice")}</a></p>
  </article>;
}
