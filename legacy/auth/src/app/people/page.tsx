import {LiveSearchForm} from "@webdock/search/form";
import { authLabel } from "@/lib/i18n-labels";
import { getRequestI18n } from "@webdock/i18n/next";
import { headers } from "next/headers";
import { redirect } from "next/navigation";
import Link from "next/link";
import { auth } from "@/lib/auth";
import { currentMCPClaims } from "@/lib/mcp";
import { listAccess } from "@/lib/access-management";
import { AccessForm, RoleSelect } from "./form";
export async function generateMetadata() { const { t } = await getRequestI18n(); return { title: t("People & access") }; }
export default async function People({
  searchParams,
}: {
  searchParams: Promise<{ q?: string }>;
}) {
 const { t, date: formatDate } = await getRequestI18n();
  const requestHeaders = await headers();
  const session = await auth.api.getSession({ headers: requestHeaders });
  if (!session) redirect("/sign-in");
  if ((await currentMCPClaims(session.user.id)).disabled) redirect("/account");
  const raw = (await searchParams).q;
  const q = typeof raw === "string" ? raw.slice(0, 160) : "";
  const data = await listAccess(requestHeaders, q);
  return (
    <div className="access-page">
      <header className="account-heading">
        <div>
          <h1>{t("People & access")}</h1>
          <p className="muted">
            {t("Invite customers and control access to each website.")}</p>
        </div>
        <Link className="button secondary" href="/account">
          {t("My account")}</Link>
      </header>
      <p>
        <a
          href={
            process.env.NEXT_PUBLIC_ADMIN_URL || "https://studio.webdock.dev"
          }
        >
          {t("Back to Studio")}</a>
      </p>
      <div className="access-grid">
        <section className="auth-panel">
          <h2>{t("Invite to a website")}</h2>
          <p>
            {t("New customers receive a secure link to choose their password. Existing accounts receive an invitation. Website access starts when customer membership is accepted.")}</p>
          <AccessForm action="invite" label={t("Save access and send invitation")}>
            <label className="field">
              {t("Customer name")}<input name="name" required maxLength={160} autoComplete="name" />
            </label>
            <label className="field">
              {t("Email address")}<input
                name="email"
                type="email"
                required
                maxLength={254}
                autoComplete="email"
              />
            </label>
            <label className="field">
              {t("Website")}<select name="binding" required defaultValue="">
                <option value="" disabled>
                  {t("Select a website")}</option>
                {data.websites
                  .filter((site) => site.organization_id)
                  .map((site) => (
                    <option key={site.id} value={site.id}>
                      {site.label} —{" "}
                      {data.organizations.find(
                        (org) => org.id === site.organization_id,
                      )?.name || t("Customer")}
                    </option>
                  ))}
              </select>
            </label>
            <RoleSelect />
          </AccessForm>
          <p className="help">
            {t("These roles never grant Studio, infrastructure or platform-operator access. Sending again updates the selected website role and resends a pending invitation.")}</p>
        </section>
        <section className="auth-panel">
          <h2>{t("Customers & websites")}</h2>
          <p>
            {t("Group website access by customer. Public sign-up remains closed.")}</p>
          <AccessForm
            action="create-organization"
            label={t("Create customer group")}
          >
            <label className="field">
              {t("Customer group name")}<input name="name" required maxLength={160} />
            </label>
          </AccessForm>
          {data.websites.map((site) => (
            <article className="access-record" key={site.id}>
              <h3>{site.label}</h3>
              {site.organization_id ? (<p className="muted">
                  {data.organizations.find(
                    (org) => org.id === site.organization_id,
                  )?.name || site.organization_id}
                </p>) : (<AccessForm action="link-website" label={t("Assign customer")}>
                  <input type="hidden" name="binding" value={site.id} />
                  <label className="field">
                    {t("Customer")}<select name="organization" required defaultValue="">
                      <option value="" disabled>
                        {t("Select a customer")}</option>
                      {data.organizations.map((org) => (
                        <option key={org.id} value={org.id}>
                          {org.name}
                        </option>
                      ))}
                    </select>
                  </label>
                </AccessForm>)}
            </article>
          ))}
        </section>
      </div>
      <LiveSearchForm className="access-search" path="/people">
        <label className="field">
          {t("Find a person")}<input
            type="search"
            name="q"
            defaultValue={q}
            data-search-default=""
            autoComplete="off"
            maxLength={160}
            placeholder={t("Name or email")}
          />
        </label>
        <button className="button">{t("Search")}</button>
        {q && <Link href="/people">{t("Clear")}</Link>}
      </LiveSearchForm>
      <section className="account-section">
        <h2>{t("Accounts")}</h2>
        <p className="muted">
          {t("Up to 100 matching accounts. Platform operators are protected from suspension here.")}</p>
        <div className="access-grid">
          {data.users.map((user) => (
            <article className="access-record" key={user.id}>
              <h3>{user.name}</h3>
              <p>
                {user.email}
                <br />
                <small>
                  {user.role === "operator" ? t("Platform operator") : user.banned ? t("Suspended") : !user.emailVerified || user.mustChangePassword ? t("Setup pending") : t("Active")}
                </small>
              </p>
              {user.role !== "operator" && (<AccessForm
                  action="suspend"
                  label={user.banned ? t("Restore account") : t("Suspend account")}
                  confirm={
                    user.banned ? undefined : t("Suspend access to all websites and sign out this person")
                  }
                >
                  <input type="hidden" name="user" value={user.id} />
                  <input
                    type="hidden"
                    name="blocked"
                    value={String(!user.banned)}
                  />
                </AccessForm>)}
            </article>
          ))}
        </div>
        {!data.users.length && <p>{t("No matching accounts.")}</p>}
      </section>
      <section className="account-section">
        <h2>{t("Website permissions")}</h2>
        <p className="muted">
          {t("Up to 200 matching permissions. Customer membership and an active account are also required.")}</p>
        <div className="access-grid">
          {data.grants.map((grant) => (
            <article className="access-record" key={grant.id}>
              <h3>{grant.label}</h3>
              <p>
                {grant.name}
                <br />
                {grant.email}
                <br />
                <small>{grant.enabled ? t("Enabled") : t("Revoked")}</small>
              </p>
              <AccessForm
                action="change-access"
                label={grant.enabled ? t("Update role") : t("Restore access")}
              >
                <input type="hidden" name="grant" value={grant.id} />
                <RoleSelect value={grant.role} />
              </AccessForm>
              {grant.enabled && (<AccessForm
                  action="revoke-access"
                  label={t("Revoke website access")}
                  confirm={t("Remove this website access")}
                >
                  <input type="hidden" name="grant" value={grant.id} />
                </AccessForm>)}
            </article>
          ))}
        </div>
        {!data.grants.length && <p>{t("No website permissions yet.")}</p>}
      </section>
      <section className="account-section">
        <h2>{t("Invitations")}</h2>
        <p className="muted">
          {t("Up to 100 matching invitations. Resend from the invitation form above.")}</p>
        {data.invitations.map((invite) => (
          <article className="access-record" key={invite.id}>
            <h3>{invite.email}</h3>
            <p>
              {invite.name} ·{" "}
              {invite.status === "pending" &&
              new Date(invite.expiresAt) < new Date() ? t("Expired") : authLabel(invite.status, t)}
            </p>
            {invite.status === "pending" && (<AccessForm
                action="cancel-invite"
                label={t("Cancel invitation")}
                confirm={t("Cancel this invitation and withdraw pending website access for this customer group")}
              >
                <input type="hidden" name="id" value={invite.id} />
              </AccessForm>)}
          </article>
        ))}
      </section>
      <section className="account-section">
        <h2>{t("Recent access changes")}</h2>
        {data.events.map((event) => (
          <p key={event.id} className="access-event">
            <time dateTime={new Date(event.created_at).toISOString()}>
              {formatDate(event.created_at, { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" })}{" "}
              {t("UTC")}</time>{" "}
            · {event.action} · {authLabel(event.outcome, t)}
            <br />
            <small>{event.actor || event.actor_id}</small>
          </p>
        ))}
      </section>
    </div>
  );
}
