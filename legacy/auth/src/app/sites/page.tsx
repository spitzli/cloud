
import {withAuthenticatedDatabase} from '@/lib/database-context';
import { authLabel } from "@/lib/i18n-labels";
import { getRequestI18n } from "@webdock/i18n/next";
import { headers } from "next/headers";
import { redirect } from "next/navigation";
import Link from "next/link";
import { auth } from "@/lib/auth";
import { listTenantInvitations } from "@/lib/tenants";
import { accountSites } from "@/lib/access-management";
export async function generateMetadata() { const { t } = await getRequestI18n(); return { title: t("Your websites") }; }
export default async function Sites() {

 const { t } = await getRequestI18n();
  const requestHeaders = await headers();
  const session = await auth.api.getSession({ headers: requestHeaders });
  if (!session) redirect("/sign-in?returnTo=sites");
  if (!session.user.emailVerified || session.user.mustChangePassword)
    redirect("/account");
  return withAuthenticatedDatabase(await headers(), async () => {
  const [sites, invites] = await Promise.all([
    accountSites(requestHeaders),
    listTenantInvitations(requestHeaders),
  ]);
  return (
    <section className="account">
      <h1>{t("Your websites")}</h1>
      <p>{t("Open a website to edit its content with your Webdock account.")}</p>
      {invites.map((invite) => (
        <article className="access-record" key={invite.id}>
          <h2>{t("Invitation to ")}{invite.name}</h2>
          <Link
            className="button"
            href={"/invitation?id=" + encodeURIComponent(invite.id)}
          >
            {t("Review invitation")}</Link>
        </article>
      ))}
      {sites.map((site) => (
        <article className="access-record" key={site.id}>
          <h2>{site.name}</h2>
          <p className="muted">{authLabel(site.role, t)}</p>
          <a className="button" href={site.url}>
            {t("Open content manager")}</a>
        </article>
      ))}
      {!sites.length && (<p className="notice">
          {t("No website access yet. Accept a pending invitation above or contact your administrator.")}</p>)}
      <p>
        <Link href="/account">{t("My account")}</Link>
      </p>
    </section>
  );

});
}
