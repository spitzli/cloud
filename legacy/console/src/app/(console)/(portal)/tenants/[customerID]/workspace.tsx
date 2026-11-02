import { ConsoleIcon } from "@/components/console-icon";
import {uiLabel} from "@/lib/ui-labels";

import { getRequestI18n } from '@webdock/i18n/next';
import { headers } from "next/headers";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";
import { auth } from "@/lib/auth";
import { getTenant, profileFields } from "@/lib/tenants";
import { AccessError } from "@/lib/access-management";
import { TenantForm, TenantRole } from "../form";
export async function generateMetadata(){ const i18n = await getRequestI18n(); return { title: i18n.t("Workspace details") }; }
export async function TenantWorkspace({
  params, view = "overview",
}: {
  params: Promise<{ customerID: string }>; view?: "overview" | "profile" | "team";
}) {
  const i18n = await getRequestI18n();

  const { customerID } = await params,
    requestHeaders = await headers();
  const session = await auth.api.getSession({ headers: requestHeaders });
  if (!session)
    redirect(
      `/api/sso/login?returnTo=${encodeURIComponent(`/tenants/${customerID}`)}`,
    );
  let data;
  try {
    data = await getTenant(requestHeaders, customerID);
  } catch (error) {
    if (error instanceof AccessError) notFound();
    throw error;
  }
  const { tenant, canManage, operator, members, invitations } = data;
  const owner = operator || tenant.member_role === "owner" || (data.team?.provider==="clerk"&&tenant.member_role==="admin");
  if(view==="team" && data.team && !data.team.ready)return <section className="account-section"><h1>{i18n.t("Team")}</h1><p>{i18n.t("Setup pending")}</p>{operator&&<TenantForm customer={customerID} action="link-team" label={i18n.t("Connect")}/>}</section>;
  if(view==="team" && data.team && !data.team.canManage)return <section className="account-section"><h1>{i18n.t("Team")}</h1>{members.map(member=><p key={member.id}>{member.name} · {i18n.t(uiLabel(member.role))}</p>)}</section>;
  const available = (module:string) => ["enabled", "draining"].includes(session.installation?.modules[module] ?? "");
  return (
    <div className="access-page">
      <header className="account-heading">
        <div>
          <h1>{view === "profile" ? i18n.t("Customer profile") : view === "team" ? i18n.t("Team") : i18n.t("Overview")}</h1>
          <p className="muted">
            {tenant.status === "active"
              ? tenant.name
              : i18n.t("Archived tenant — changes disabled")}
          </p>
        </div>
      </header>
      {view === "profile" && <div className="access-grid">
        <section className="auth-panel">
          <h2>{i18n.t("Customer profile")}</h2>
          {canManage ? (
            <TenantForm
              customer={customerID}
              action="profile"
              label={i18n.t("Save profile")}
            >
              <label className="field">{i18n.t("Customer type")}<select
                  name="customer_type"
                  defaultValue={tenant.customer_type}
                >
                  <option value="person">{i18n.t("Person")}</option>
                  <option value="company">{i18n.t("Company")}</option>
                </select>
              </label>
              <div className="profile-fields">
                {profileFields
                  .filter(([field]) =>
                    ["name", "contact_name", "contact_email", "phone"].includes(
                      field,
                    ),
                  )
                  .map(([field, label, max]) => (
                    <label className="field" key={field}>
                      {i18n.t(label)}
                      <input
                        name={field}
                        defaultValue={tenant[field] || ""}
                        required={field === "name"}
                        maxLength={max}
                        type={
                          field === "contact_email"
                            ? "email"
                            : field === "phone"
                              ? "tel"
                              : "text"
                        }
                      />
                    </label>
                  ))}
              </div>
              <details>
                <summary>{i18n.t("Company, personal details & address")}</summary>
                <div className="profile-fields">
                  {profileFields
                    .filter(
                      ([field]) =>
                        ![
                          "name",
                          "contact_name",
                          "contact_email",
                          "phone",
                        ].includes(field),
                    )
                    .map(([field, label, max]) => (
                      <label className="field" key={field}>
                        {i18n.t(label)}
                        <input
                          name={field}
                          defaultValue={tenant[field] || ""}
                          maxLength={max}
                        />
                      </label>
                    ))}
                </div>
              </details>
              <p className="help">{i18n.t("Contact details do not change anyone’s sign-in email or permissions.")}</p>
            </TenantForm>
          ) : (
            <dl>
              <dt>{i18n.t("Customer type")}</dt>
              <dd>
                {tenant.customer_type === "company" ? i18n.t("Company") : i18n.t("Person")}
              </dd>
              {profileFields.filter(([field]) => field !== "name" && tenant[field]).map(([field, label]) => (
                <div key={field}>
                  <dt>{i18n.t(label)}</dt>
                  <dd>{tenant[field]}</dd>
                </div>
              ))}
            </dl>
          )}
        </section>
      </div>}
      {view === "overview" && <>

        <section className="account-section"><h2>{i18n.t("Services")}</h2><div className="console-services">
          {available("hosting") && <Link href={`/tenants/${customerID}/hosting`}><ConsoleIcon name="apps"/><strong>{i18n.t("Applications")}</strong><small>{i18n.t("Deploy applications and inspect their status and logs.")}</small><span aria-hidden="true">›</span></Link>}
          {available("databases") && <Link href={`/tenants/${customerID}/${session.preview?'managed-databases':'databases'}`}><ConsoleIcon name="database"/><strong>{i18n.t("Databases")}</strong><small>{i18n.t("Browse project data and manage database connections.")}</small><span aria-hidden="true">›</span></Link>}
          {available("mail")&&!session.preview&&<Link href={`/tenants/${customerID}/mail`}><ConsoleIcon name="mail"/><strong>{i18n.t("Mail")}</strong><small>{i18n.t("Manage sending, domains and mailboxes.")}</small><span aria-hidden="true">›</span></Link>}
          <Link href={`/tenants/${customerID}/usage`}><ConsoleIcon name="billing"/><strong>{i18n.t("Plan & usage")}</strong><small>{i18n.t("Check your plan, resource limits and measured usage.")}</small><span aria-hidden="true">›</span></Link>
        </div></section>
      </>}
      {view === "team" && canManage && (
        <>

          <section className="account-section">
            <h2>{i18n.t("People")}</h2>
            <p className="muted">{data.team ? i18n.t("Administrators manage members and invitations. Clerk applies its team rules.") : i18n.t("Admins manage members. Owners manage admins and owners. Removing membership immediately removes workspace access.")}</p>
            <div className="access-grid">
              {members.map((member) => (
                <article className="access-record" key={member.id}>
                  <h3>{member.name}</h3>
                  <p>
                    {member.email}
                    <br />
                    {i18n.t(uiLabel(member.role))} ·{" "}
                    {member.banned
                      ? i18n.t("Suspended")
                      : !member.emailVerified || member.mustChangePassword
                        ? i18n.t("Setup pending")
                        : i18n.t("Active")}
                  </p>
                  <p className="muted">{["admin","owner"].includes(member.role) ? i18n.t("Website administrator rights come from this workspace role. Explicitly revoked website access still blocks access.") : i18n.t("Website permissions are assigned individually. Workspace membership alone does not grant editing rights.")}</p>
                  {(owner || member.role === "member") && (
                    <details>
                      <summary>{i18n.t("Manage membership")}</summary>
                      <TenantForm
                        customer={customerID}
                        action="role"
                        label={i18n.t("Update tenant role")}
                      >
                        <input type="hidden" name="member" value={member.id} />
                        <TenantRole value={member.role} allowOwner={owner&&!data.team} memberOnly={!owner} />
                      </TenantForm>
                      <TenantForm
                        customer={customerID}
                        action="remove"
                        label={i18n.t("Remove membership")}
                        confirm={i18n.t("Remove this person’s tenant and website access")}
                      >
                        <input type="hidden" name="member" value={member.id} />
                      </TenantForm>
                    </details>
                  )}
                </article>
              ))}
            </div>
            {!members.length && (
              <p>{i18n.t("No customer members yet. Invite the first person with the Administrator role.")}</p>
            )}
          </section>
          <details className="workspace-disclosure">
            <summary>{i18n.t("Invite a person")}</summary>
            <p>{i18n.t("New people receive a secure link to choose their password. Tenant access starts after they accept the invitation.")}</p>
            <TenantForm
              customer={customerID}
              action="invite"
              label={i18n.t("Send invitation")}
            >
              <label className="field">{i18n.t("Name")}<input name="name" required maxLength={160} />
              </label>
              <label className="field">{i18n.t("Email")}<input name="email" type="email" required maxLength={254} />
              </label>
              <TenantRole
                value={!data.team && operator && members.length === 0 ? "admin" : "member"}
              />
            </TenantForm>
          </details>
          <section className="account-section">
            <h2>{i18n.t("Invitations")}</h2>
            {invitations.map((invite) => (
              <article className="access-record" key={invite.id}>
                <h3>{invite.email}</h3>
                <p>
                  {i18n.t(uiLabel(invite.role))} ·{" "}
                  {invite.status === "pending" &&
                  new Date(invite.expiresAt) < new Date()
                    ? i18n.t("Expired")
                    : i18n.t(uiLabel(invite.status))}
                </p>
                {invite.status === "pending" && (
                  <>
                    <TenantForm
                      customer={customerID}
                      action="resend"
                      label={i18n.t("Resend invitation")}
                    >
                      <input
                        type="hidden"
                        name="invitation"
                        value={invite.id}
                      />
                    </TenantForm>
                    <TenantForm
                      customer={customerID}
                      action="cancel"
                      label={i18n.t("Cancel invitation")}
                      confirm={i18n.t("Cancel the invitation and pending website access")}
                    >
                      <input
                        type="hidden"
                        name="invitation"
                        value={invite.id}
                      />
                    </TenantForm>
                  </>
                )}
              </article>
            ))}
            {!invitations.length && <p>{i18n.t("No invitations yet.")}</p>}
          </section>
        </>
      )}
    </div>
  );
}

export default TenantWorkspace;
