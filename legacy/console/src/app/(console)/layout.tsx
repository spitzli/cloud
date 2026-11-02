import { ProductProvider } from "@distribution/providers";
import { deDE, enUS } from "@clerk/localizations";
import { ClerkProvider } from "@clerk/nextjs";
import { browserAuthConfiguration, browserAuthPageURL } from "@/lib/browser-auth-config";
import { connection } from "next/server";
import { ModuleProvider } from "@webdock/module-sdk/react";
import { getStudioInstallation } from "@/lib/studio-client";
import { I18nProvider } from "@webdock/i18n/react";
import { getRequestI18n } from "@webdock/i18n/next";
import type { Metadata } from "next";
import localFont from "next/font/local";
import "./styles.css";
import "./console.css";
const space = localFont({
  src: "../../../public/fonts/space-grotesk.ttf",
  variable: "--font-space",
  display: "swap",
});
export const metadata: Metadata = {
  title: { default: "Webdock Studio", template: "%s — Webdock" },
  robots: { index: false, follow: false },
};
export default async function Layout({
  children,
  toolbar,
  navigation,
}: {
  children: React.ReactNode;
  toolbar: React.ReactNode;
  navigation: React.ReactNode;
}) {
  await connection();
  const installation = await getStudioInstallation();
  const { locale, preference, t } = await getRequestI18n();
  const content = (
    <I18nProvider locale={locale} preference={preference}>
      <ProductProvider>
        <ModuleProvider view={installation}>
          <div className="console-layout">
            <a className="skip-link" href="#main">
              {t("Skip to content")}
            </a>
            {toolbar}
            {navigation}
            {children}
          </div>
        </ModuleProvider>
      </ProductProvider>
    </I18nProvider>
  );
  return (
    <html lang={locale} className={space.variable} suppressHydrationWarning>
      <head>
        <script
          dangerouslySetInnerHTML={{
            __html:
              "try{const t=localStorage.getItem('webdock-theme');if(t==='light'||t==='dark')document.documentElement.dataset.theme=t}catch{}",
          }}
        />
      </head>
      <body>
        {browserAuthConfiguration.provider === "clerk" ? (
          <ClerkProvider
            publishableKey={
              process.env.CLERK_PUBLISHABLE_KEY ||
              process.env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY
            }
            localization={locale === "de" ? deDE : enUS}
            signInUrl={browserAuthPageURL("/login")}
            signUpUrl={browserAuthPageURL("/sign-up")}
            signInFallbackRedirectUrl="/tenants"
          >
            {content}
          </ClerkProvider>
        ) : (
          content
        )}
      </body>
    </html>
  );
}
