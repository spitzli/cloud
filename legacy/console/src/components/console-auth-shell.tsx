import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";

export async function ConsoleAuthShell({ title, description, children, showIntro = true }: {
  title: string; description: string; children: React.ReactNode; showIntro?: boolean;
}) {
  const { t } = await getRequestI18n();
  return <main id="main" className="console-auth-screen">
    <div className="console-auth-container">
      <a className="console-auth-brand" href="https://webdock.dev" aria-label={t("Webdock home")}>webdock<span>.</span></a>
      <section className="console-auth-card" aria-labelledby={showIntro ? "auth-heading" : undefined} aria-label={showIntro ? undefined : t(title)}>
        {showIntro && <header><h1 id="auth-heading">{t(title)}</h1><p>{t(description)}</p></header>}
        {children}
      </section>
      <nav className="console-auth-footer" aria-label={t("Account navigation")}>
        <Link href="/login">{t("Login")}</Link><Link href="/waitlist">{t("Waitlist")}</Link><a href="https://webdock.dev/privacy">{t("Privacy")}</a>
      </nav>
    </div>
  </main>;
}

export const consoleAuthAppearance = {
  variables: {
    colorPrimary: "#ff9255", colorPrimaryForeground: "#1b293c",
    colorBackground: "var(--panel)", colorForeground: "var(--ink)",
    colorMutedForeground: "var(--muted)", colorInput: "var(--bg)",
    colorInputForeground: "var(--ink)", colorBorder: "var(--line)",
    fontFamily: "var(--font-space), sans-serif", borderRadius: "8px",
  },
  elements: {
    rootBox: { width: "100%" }, cardBox: { width: "100%", boxShadow: "none", border: "none" },
    card: { background: "transparent", boxShadow: "none", padding: "0", border: "none" },
    logoBox: { display: "none" },
    headerTitle: { fontSize: "24px" },
    footer: { background: "transparent" },
    formButtonPrimary: { minHeight: "44px" },
  },
};
