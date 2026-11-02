import { getRequestI18n } from "@webdock/i18n/next";
import type { Metadata } from "next";
import { headers } from "next/headers";
import Link from "next/link";
import { auth } from "@/lib/auth";
import { AuthPanel } from "@/components/auth-forms";
import { Invitation } from "@/components/invitation";
import { businessURL } from "@/lib/studio-links";

export async function generateMetadata(): Promise<Metadata> { const { t } = await getRequestI18n(); return { title: t("Organization invitation") }; }

export default async function InvitationPage({ searchParams }: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
 const { t } = await getRequestI18n();
  const { id } = await searchParams;
  if (typeof id !== "string" || !id || id.length > 128) return <AuthPanel title={t("Invitation unavailable")}><p>{t("This invitation link is incomplete. Open the original link or contact your Webdock administrator.")}</p></AuthPanel>;
  const session = await auth.api.getSession({ headers: await headers() });
  if (!session) return <AuthPanel title={t("Sign in to view your invitation")}>
    <p className="muted">{t("Sign in with your existing Webdock account, then review this invitation.")}</p>
    <Link className="button" href={`/sign-in?invitation=${encodeURIComponent(id)}`}>{t("Sign in")}</Link>
    <p className="help">{t("If this is your first visit, use the password setup link in your invitation email. Ask your administrator to resend it if the link has expired.")}</p>
  </AuthPanel>;
  if (!session.user.emailVerified) return <AuthPanel title={t("Verify your email first")}><p>{t("Your account’s email address must be verified before you can view or respond to this invitation. Contact your Webdock administrator for help.")}</p><Link href="/account">{t("Go to your account")}</Link></AuthPanel>;
  return <Invitation id={id} tenantsURL={businessURL("/tenants")} />;
}
