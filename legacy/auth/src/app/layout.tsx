import { getRequestI18n } from "@webdock/i18n/next";
import { I18nProvider } from "@webdock/i18n/react";
import { LanguagePicker } from "@webdock/i18n/picker";
import type { Metadata } from "next";
import localFont from "next/font/local";
import Link from "next/link";
import "./styles.css";
import { publicPlatformSettings } from "@/lib/platform";
import { studioURL } from "@/lib/studio-links";

export const dynamic = "force-dynamic";

const space = localFont({
  src: "../../public/fonts/space-grotesk.ttf",
  variable: "--font-space",
  display: "swap",
});
export async function generateMetadata(): Promise<Metadata> { const { t } = await getRequestI18n(); return {
  title: { default: t("Webdock Account"), template: "%s — Webdock" },
  robots: { index: false, follow: false },
  referrer: "same-origin",
}; }

export default async function Layout({ children }: { children: React.ReactNode }) {
 const { t, locale, preference } = await getRequestI18n();
  const platform = await publicPlatformSettings();
  return (
    <html lang={locale} className={space.variable}>
      <body><I18nProvider locale={locale} preference={preference}>
        <a className="skip-link" href="#main">
          {t("Skip to content")}</a>
        <header className="site-header">
          <Link className="brand" href="/">
            {platform.name}<span>.</span>
          </Link>
          <nav aria-label={t("Account")}>
            <Link href="/account">{t("Account & security")}</Link>
            <Link href="/connections">{t("Connected apps")}</Link>
            <a href={studioURL()}>{t("Open Studio")}</a>
          </nav><LanguagePicker className="auth-language-picker"/>
        </header>
        <main id="main">{children}</main>
        <footer className="site-footer">
          {t("Your {name} sign-in and account security.", {name: platform.name})}{" "}
          <a href={`mailto:${platform.supportEmail}`}>{t("Support")}</a>{" · "}
          <a href="https://webdock.dev/privacy">{t("Privacy")}</a>
        </footer>
      </I18nProvider></body>
    </html>
  );
}
