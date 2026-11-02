import { soleWorkspace } from "@/lib/workspace-navigation";
import {uiLabel} from "@/lib/ui-labels";

import { getRequestI18n } from '@webdock/i18n/next';
import { headers } from "next/headers";
import Link from "next/link";
import { redirect } from "next/navigation";
import { auth } from "@/lib/auth";
import { listTenants, listTenantInvitations } from "@/lib/tenants";
import { AccessError } from "@/lib/access-management";
export async function generateMetadata(){ const i18n = await getRequestI18n(); return { title: i18n.t("Your workspaces") }; }
export default async function Tenants({searchParams}: {searchParams: Promise<{choose?: string}>}) {
  const i18n = await getRequestI18n();

  const requestHeaders = await headers();
  const session = await auth.api.getSession({ headers: requestHeaders });
  if (!session)
    redirect("/api/sso/login?returnTo=%2Ftenants");
  let tenants, invitations;
  try {
    [tenants, invitations] = await Promise.all([
      listTenants(requestHeaders),
      listTenantInvitations(requestHeaders),
    ]);
  } catch (error) {
    if (error instanceof AccessError)
      return (
        <section className="auth-panel">
          <h1>{i18n.t("Workspaces")}</h1>
          <p>{i18n.t("You do not currently have tenant access. Accept your emailed invitation or contact your Webdock operator.")}</p>
        </section>
      );
    throw error;
  }
  const landing = soleWorkspace(tenants, invitations.length, session.user.role === "operator");
  if (landing && (await searchParams).choose !== "1") redirect(landing);
  return (
    <div className="access-page">
      <header className="account-heading">
        <div>
          <h1>{i18n.t("Your workspaces")}</h1>
          <p className="muted">{i18n.t("Customer details, people and assigned websites.")}</p>
        </div>
        <Link
          className="button secondary"
          href={
            new URL(
              "/account",
              process.env.WEBDOCK_AUTH_ISSUER ||
                "https://auth.webdock.dev/api/auth",
            ).href
          }
        >{i18n.t("Account & security")}</Link>
      </header>
      {invitations.map((invite) => (
        <article className="access-record" key={invite.id}>
          <h2>{i18n.t("Invitation to ")}{invite.name}</h2>
          <p className="muted">{i18n.t("Workspace role: ")}{i18n.t(uiLabel(invite.role))}</p>
          <Link
            className="button"
            href={
              new URL(
                `/invitation?id=${encodeURIComponent(invite.id)}`,
                process.env.WEBDOCK_AUTH_ISSUER ||
                  "https://auth.webdock.dev/api/auth",
              ).href
            }
          >{i18n.t("Review invitation")}</Link>
        </article>
      ))}
      {tenants.length > 0 && <div className="console-directory"><table>
        <thead><tr><th scope="col">{i18n.t("Workspace")}</th><th scope="col">{i18n.t("Access")}</th><th scope="col" className="console-directory-id">{i18n.t("Workspace ID")}</th><th scope="col">{i18n.t("Actions")}</th></tr></thead>
        <tbody>{tenants.map(tenant=><tr key={tenant.id}><td><Link href={`/tenants/${tenant.id}`}>{tenant.name}</Link></td><td>{tenant.status === "active" ? tenant.role ? i18n.t(uiLabel(tenant.role)) : i18n.t("Platform operator") : i18n.t("Archived")}</td><td className="console-directory-id"><code>{tenant.id}</code></td><td><Link className="button secondary" href={`/tenants/${tenant.id}`}>{i18n.t("Open workspace")}</Link></td></tr>)}</tbody>
      </table></div>}
      {!tenants.length && (
        <p>{i18n.t("No active tenant memberships yet. Accept your emailed invitation to get started.")}</p>
      )}
      <p>

      </p>
    </div>
  );
}
