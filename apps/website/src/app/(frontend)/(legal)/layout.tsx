import {LanguagePicker} from '@webdock/i18n/picker';
import {getRequestI18n} from '@webdock/i18n/next';
import Link from 'next/link';
import { ThemePicker } from '@/components/theme-picker';

export default async function LegalLayout({ children }: { children: React.ReactNode }) {
  const {t}=await getRequestI18n();
  return <>
    <a className="skip-link" href="#content">{t("Skip to content")}</a>
    <header className="site-header wrap flex items-center justify-between">
      <Link className="brand" href="/" aria-label={t("Spitzli Cloud home")}>{"Spitzli Cloud"}</Link>
      <div className="preference-pickers"><ThemePicker /><LanguagePicker className="language-picker"/></div>
    </header>
    <main id="content" className="wrap legal-copy">{children}</main>
    <footer className="wrap site-footer"><Link href="/">{"Spitzli Cloud"}</Link><nav aria-label={t("Legal")}><a href="/terms">{t("Terms")}</a><a href="/privacy">{t("Privacy")}</a><a href="/imprint">{t("Legal notice")}</a></nav><a href="mailto:dominik@spitzli.dev">{"dominik@spitzli.dev"}</a></footer>
  </>;
}
