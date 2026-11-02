import { getRequestI18n } from "@webdock/i18n/next";
import type { Metadata } from "next";
import { studioURL } from "@/lib/studio-links";
import { TwoFactorForm } from "@/components/auth-forms";
export async function generateMetadata(): Promise<Metadata> { const { t } = await getRequestI18n(); return { title: t("Verify sign-in") }; }
export default function TwoFactor() {
  return <TwoFactorForm restartURL={studioURL("/api/sso/login")} />;
}
