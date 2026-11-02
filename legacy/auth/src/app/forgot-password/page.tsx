import { getRequestI18n } from "@webdock/i18n/next";
import type { Metadata } from "next";
import { ForgotPasswordForm } from "@/components/auth-forms";

export async function generateMetadata(): Promise<Metadata> { const { t } = await getRequestI18n(); return { title: t("Reset your password") }; }
export default function ForgotPassword() {
  return <ForgotPasswordForm />;
}
