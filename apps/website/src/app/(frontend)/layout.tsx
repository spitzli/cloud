import Script from 'next/script';
import analytics from '@/lib/managed-analytics.json';
import {I18nProvider} from '@webdock/i18n/react';
import {getRequestI18n} from '@webdock/i18n/next';
import type { Metadata } from 'next';
import localFont from 'next/font/local';
import '../globals.css';
import { themeScript } from '@/lib/theme';
const space = localFont({ src: '../../../public/fonts/space-grotesk-latin-variable.woff2', variable: '--font-space', display: 'swap' });
const mono = localFont({ src: '../../../public/fonts/jetbrains-mono-latin-variable.woff2', variable: '--font-mono', display: 'swap' });
export async function generateMetadata():Promise<Metadata>{const {t,locale}=await getRequestI18n();return {
  metadataBase: new URL('https://spitzli.cloud'),
  title: t("Spitzli Cloud — Websites, applications and cloud services"),
  description: t("Your projects in one place. Websites and applications by Spitzli — on spitzli.site or your own domain."),
  alternates: { canonical: '/' },
  robots: { index: true, follow: true },
  openGraph: { type: 'website', locale: locale === 'de' ? 'de_DE' : 'en_GB', url: '/', siteName: 'Spitzli Cloud', title: t("Spitzli Cloud — Your projects. One place."), description: t("Websites, applications and cloud services by Spitzli Development."), images: [{ url: '/og', width: 1200, height: 630, alt: t("Spitzli Cloud — Your projects. One place.") }] },
  twitter: { card: 'summary_large_image', images: ['/og'] },
  icons: { icon: '/icon.svg' },
};}
export default async function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  const {locale,preference}=await getRequestI18n();
  return <html lang={locale} className={`${space.variable} ${mono.variable}`} suppressHydrationWarning><head><script dangerouslySetInnerHTML={{ __html: themeScript }}/></head><body><I18nProvider locale={locale} preference={preference}>{children}</I18nProvider>{process.env.VERCEL_ENV === "production" && analytics.enabled && <Script id="managed-analytics" src={analytics.scriptURL} strategy="afterInteractive" data-website-id={analytics.websiteID} data-domains={analytics.domains.join(",")} data-exclude-search="true" data-exclude-hash="true" data-do-not-track="true"/>}</body></html>;
}
