import { ConsoleAuthShell } from "@/components/console-auth-shell";
import { getRequestI18n } from "@webdock/i18n/next";
export default async function WaitlistConfirmed() {
  const { t } = await getRequestI18n();
  return <ConsoleAuthShell title="You’re on the waitlist" description="Thanks for your interest in Webdock. We will email you when we can invite you.">
    <p className="console-auth-confirmation">{t("You do not need to sign in yet. Access starts when you receive your invitation.")}</p>
    <a className="console-auth-back" href="https://webdock.dev">{t("Back to homepage")}</a>
  </ConsoleAuthShell>;
}
