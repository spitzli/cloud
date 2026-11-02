import {browserAuthConfiguration} from "@/lib/browser-auth-config";
import { AreaNavigation } from "@/components/area-navigation";
import {LiveSearchForm} from "@webdock/search/form";
import {uiLabel} from "@/lib/ui-labels";

import { getRequestI18n } from '@webdock/i18n/next';
import { headers } from "next/headers";
import { redirect } from "next/navigation";
import Link from "next/link";
import { auth } from "@/lib/auth";
import { listAccess } from "@/lib/access-management";
import { AccessForm, RoleSelect } from "./form";
export async function generateMetadata(){ const i18n = await getRequestI18n(); return { title: i18n.t("People & access") }; }
export default async function People({
  searchParams,
}: {
  searchParams: Promise<{ q?: string; view?: string }>;
}) {
  if (browserAuthConfiguration.provider === "clerk") redirect("/admin/registry-users");
  const i18n = await getRequestI18n();

  const requestHeaders = await headers();
  const session = await auth.api.getSession({ headers: requestHeaders });
  if (!session) redirect("/api/sso/login?returnTo=%2Fpeople");
  if (session.user.role !== "operator") redirect("/tenants");
  const query = await searchParams;
  const view = ["overview","permissions","invitations","websites","history"].includes(query.view ?? "") ? query.view! : "overview";
  const raw = query.q;
  const q = typeof raw === "string" ? raw.slice(0, 160) : "";
  const data = await listAccess(requestHeaders, q);
  return (
    <div className="access-page">
      <header className="account-heading">
        <div>
          <h1>{i18n.t("People & access")}</h1>
          <p className="muted">{i18n.t("Invite customers and control access to each website.")}</p>
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
      <AreaNavigation links={[["overview",i18n.t("Accounts")],["permissions",i18n.t("Website permissions")],["invitations",i18n.t("Invitations")],["websites",i18n.t("Customers & websites")],["history",i18n.t("Recent access changes")]].map(([key,label])=>({href:"/people?"+new URLSearchParams({view:key,...(q?{q}:{})}),label,exact:true}))}/>
      <div className="access-grid">
        {view === "invitations" && <section className="auth-panel">
          <h2>{i18n.t("Invite to a website")}</h2>
          <p>{i18n.t("New customers receive a secure link to choose their password. Existing accounts receive an invitation. Website access starts when customer membership is accepted.")}</p>
          <AccessForm action="invite" label={i18n.t("Save access and send invitation")}>
            <label className="field">{i18n.t("Customer name")}<input name="name" required maxLength={160} autoComplete="name" />
            </label>
            <label className="field">{i18n.t("Email address")}<input
                name="email"
                type="email"
                required
                maxLength={254}
                autoComplete="email"
              />
            </label>
            <label className="field">{i18n.t("Website")}<select name="binding" required defaultValue="">
                <option value="" disabled>{i18n.t("Select a website")}</option>
                {data.websites
                  .filter((site) => site.organization_id)
                  .map((site) => (
                    <option key={site.id} value={site.id}>
                      {site.label} —{" "}
                      {data.organizations.find(
                        (org) => org.id === site.organization_id,
                      )?.name || i18n.t("Customer")}
                    </option>
                  ))}
              </select>
            </label>
            <RoleSelect />
          </AccessForm>
          <p className="help">{i18n.t("Website roles do not grant infrastructure or platform-operator access. Sending again updates the selected website role and resends a pending invitation.")}</p>
        </section>}
        {view === "websites" && <section className="auth-panel">
          <h2>{i18n.t("Customers & websites")}</h2>
          <p>{i18n.t("Group website access by customer. Public sign-up remains closed.")}</p>
          <AccessForm
            action="create-organization"
            label={i18n.t("Create customer group")}
          >
            <label className="field">{i18n.t("Customer group name")}<input name="name" required maxLength={160} />
            </label>
          </AccessForm>
          {data.websites.map((site) => (
            <article className="access-record" key={site.id}>
              <h3>{site.label}</h3>
              {site.organization_id ? (
                <p className="muted">
                  {data.organizations.find(
                    (org) => org.id === site.organization_id,
                  )?.name || site.organization_id}
                </p>
              ) : (
                <AccessForm action="link-website" label={i18n.t("Assign customer")}>
                  <input type="hidden" name="binding" value={site.id} />
                  <label className="field">{i18n.t("Customer")}<select name="organization" required defaultValue="">
                      <option value="" disabled>{i18n.t("Select a customer")}</option>
                      {data.organizations.map((org) => (
                        <option key={org.id} value={org.id}>
                          {org.name}
                        </option>
                      ))}
                    </select>
                  </label>
                </AccessForm>
              )}
            </article>
          ))}
        </section>}
      </div>
      <LiveSearchForm className="access-search" path="/people"><input type="hidden" name="view" value={view}/>
        <label className="field">{i18n.t("Find a person")}<input
            type="search"
            name="q"
            defaultValue={q}
            data-search-default=""
            autoComplete="off"
            maxLength={160}
            placeholder={i18n.t("Name or email")}
          />
        </label>
        <noscript><button className="button">{i18n.t("Search")}</button></noscript>
        {q && <Link href={"/people?"+new URLSearchParams({view})}>{i18n.t("Clear")}</Link>}
      </LiveSearchForm>
      {view === "overview" && <section className="account-section">
        <h2>{i18n.t("Accounts")}</h2>
        <p className="muted">{i18n.t("Up to 100 matching accounts. Platform operators are protected from suspension here.")}</p>
        <div className="workspace-list">
          {data.users.map((user) => (
            <article key={user.id}>
              <h3>{user.name}</h3>
              <p>
                {user.email}
                <br />
                <small>
                  {user.role === "operator"
                    ? i18n.t("Platform operator")
                    : user.banned
                      ? i18n.t("Suspended")
                      : !user.emailVerified || user.mustChangePassword
                        ? i18n.t("Setup pending")
                        : i18n.t("Active")}
                </small>
              </p>
              {user.role !== "operator" && (
                <details>
                  <summary>{i18n.t("Manage account")}</summary>
                  <AccessForm
                    action="suspend"
                    label={user.banned ? i18n.t("Restore account") : i18n.t("Suspend account")}
                    confirm={
                      user.banned
                        ? undefined
                        : i18n.t("Suspend access to all websites and sign out this person")
                    }
                  >
                    <input type="hidden" name="user" value={user.id} />
                    <input
                      type="hidden"
                      name="blocked"
                      value={String(!user.banned)}
                    />
                  </AccessForm>
                </details>
              )}
            </article>
          ))}
        </div>
        {!data.users.length && <p>{i18n.t("No matching accounts.")}</p>}
      </section>}
      {view === "permissions" && <section className="account-section">
        <h2>{i18n.t("Website permissions")}</h2>
        <p className="muted">{i18n.t("Up to 200 matching permissions. Customer membership and an active account are also required.")}</p>
        <div className="access-grid">
          {data.grants.map((grant) => (
            <article className="access-record" key={grant.id}>
              <h3>{grant.label}</h3>
              <p>
                {grant.name}
                <br />
                {grant.email}
                <br />
                <small>{grant.enabled ? i18n.t("Enabled") : i18n.t("Revoked")}</small>
              </p>
              <AccessForm
                action="change-access"
                label={grant.enabled ? i18n.t("Update role") : i18n.t("Restore access")}
              >
                <input type="hidden" name="grant" value={grant.id} />
                <RoleSelect value={grant.role} />
              </AccessForm>
              {grant.enabled && (
                <AccessForm
                  action="revoke-access"
                  label={i18n.t("Revoke website access")}
                  confirm={i18n.t("Remove this website access")}
                >
                  <input type="hidden" name="grant" value={grant.id} />
                </AccessForm>
              )}
            </article>
          ))}
        </div>
        {!data.grants.length && <p>{i18n.t("No website permissions yet.")}</p>}
      </section>}
      {view === "invitations" && <section className="account-section">
        <h2>{i18n.t("Invitations")}</h2>
        <p className="muted">{i18n.t("Up to 100 matching invitations. Resend from the invitation form above.")}</p>
        {data.invitations.map((invite) => (
          <article className="access-record" key={invite.id}>
            <h3>{invite.email}</h3>
            <p>
              {invite.name} ·{" "}
              {invite.status === "pending" &&
              new Date(invite.expiresAt) < new Date()
                ? i18n.t("Expired")
                : i18n.t(uiLabel(invite.status))}
            </p>
            {invite.status === "pending" && (
              <AccessForm
                action="cancel-invite"
                label={i18n.t("Cancel invitation")}
                confirm={i18n.t("Cancel this invitation and withdraw pending website access for this customer group")}
              >
                <input type="hidden" name="id" value={invite.id} />
              </AccessForm>
            )}
          </article>
        ))}
      </section>}
      {view === "history" && <section className="account-section">
        <h2>{i18n.t("Recent access changes")}</h2>
        {data.events.map((event) => (
          <p key={event.id} className="access-event">
            <time dateTime={new Date(event.created_at).toISOString()}>
              {new Date(event.created_at)
                .toISOString()
                .replace("T", " ")
                .slice(0, 16)}{" "}{i18n.t("UTC")}</time>{" "}
            · {i18n.t(uiLabel(event.action))} · {i18n.t(uiLabel(event.outcome))}
            <br />
            <small>{event.actor || event.actor_id}</small>
          </p>
        ))}
      </section>}
    </div>
  );
}
