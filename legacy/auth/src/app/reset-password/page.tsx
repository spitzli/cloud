import { getRequestI18n } from "@webdock/i18n/next";
import type { Metadata } from "next";
import { ResetPasswordForm } from "@/components/auth-forms";

export async function generateMetadata(): Promise<Metadata> { const { t } = await getRequestI18n(); return { title: t("Choose a new password") }; }
export default async function ResetPassword({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const query = await searchParams;
  const token =
    !query.error &&
    typeof query.token === "string" &&
    query.token.length > 0 &&
    query.token.length <= 512
      ? query.token
      : null;
  return <ResetPasswordForm token={token} invitation={typeof query.invitation === "string" && /^[1-9][0-9]{0,18}$/.test(query.invitation) ? query.invitation : undefined} />;
}
