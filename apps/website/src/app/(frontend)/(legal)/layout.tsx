import {LanguagePicker} from '@webdock/i18n/picker';
import {getRequestI18n} from '@webdock/i18n/next';
import { IntentLink } from '@/components/intent-link';
import { ThemePicker } from '@/components/theme-picker';

export default async function LegalLayout({ children }: { children: React.ReactNode }) {
  const {t}=await getRequestI18n();
  return <>
    <a className="skip-link" href="#content">{t("Skip to content")}</a>
    <header className="site-header wrap flex items-center justify-between">
      <IntentLink className="brand" href="/" aria-label={t("Spitzli Cloud home")}>{"Spitzli Cloud"}</IntentLink>
      <div className="preference-pickers"><ThemePicker /><LanguagePicker className="language-picker"/></div>
    </header>
    <main id="content" className="wrap legal-copy">{children}</main>
    <footer className="wrap site-footer"><IntentLink href="/">{"Spitzli Cloud"}</IntentLink><nav aria-label={t("Legal")}><IntentLink href="/terms">{t("Terms")}</IntentLink><IntentLink href="/privacy">{t("Privacy")}</IntentLink><IntentLink href="/imprint">{t("Legal notice")}</IntentLink></nav><a href="mailto:dominik@spitzli.dev">{"dominik@spitzli.dev"}</a></footer>
  </>;
}
