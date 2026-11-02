import { getRequestI18n } from "@webdock/i18n/next";
import type { Metadata } from "next";
import { studioURL } from "@/lib/studio-links";
import { SignInForm } from "@/components/auth-forms";
export async function generateMetadata(): Promise<Metadata> { const { t } = await getRequestI18n(); return { title: t("Sign in") }; }
export default function SignIn() {
  return <SignInForm restartURL={studioURL("/api/sso/login")} />;
}
