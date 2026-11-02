import { authLabel } from "@/lib/i18n-labels";
import { getRequestI18n } from "@webdock/i18n/next";
import { headers } from "next/headers";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";
import { auth } from "@/lib/auth";
import { getTenant, profileFields } from "@/lib/tenants";
import { AccessError } from "@/lib/access-management";
import { TenantForm, TenantRole } from "../form";
export async function generateMetadata() { const { t } = await getRequestI18n(); return { title: t("Tenant details") }; }
export default async function Tenant({ params }: { params: Promise<{ customerID: string }> }) {
 const { t } = await getRequestI18n();
  const { customerID } = await params, requestHeaders = await headers();
  if (!(await auth.api.getSession({ headers: requestHeaders }))) redirect("/sign-in");
  let data;
  try { data = await getTenant(requestHeaders, customerID); }
  catch (error) { if (error instanceof AccessError) notFound(); throw error; }
  const { tenant, canManage, operator, sites, members, invitations } = data;
  return <div className="access-page">
    <header className="account-heading"><div><h1>{tenant.name}</h1><p className="muted">{tenant.status === "active" ? t("Customer profile and tenant access") : t("Archived tenant — changes disabled")}</p></div><Link className="button secondary" href="/tenants">{t("All tenants")}</Link></header>
    <p><Link className="button secondary" href={`/tenants/${customerID}/mail`}>{t("Mail")}</Link>{" "}<Link className="button secondary" href={`/tenants/${customerID}/usage`}>{t("Plan and usage")}</Link></p>
    <div className="access-grid"><section className="auth-panel"><h2>{t("Customer profile")}</h2>
      {canManage ? <TenantForm customer={customerID} action="profile" label={t("Save profile")}>
        <label className="field">{t("Customer type")}<select name="customer_type" defaultValue={tenant.customer_type}><option value="person">{t("Person")}</option><option value="company">{t("Company")}</option></select></label>
        {profileFields.map(([field, label, max]) => <label className="field" key={field}>{t(label)}<input name={field} defaultValue={tenant[field] || ""} required={field === "name"} maxLength={max} type={field === "contact_email" ? "email" : field === "phone" ? "tel" : "text"} /></label>)}
        <p className="help">{t("Contact details do not change anyone’s sign-in email or permissions.")}</p>
      </TenantForm> : <dl><dt>{t("Customer type")}</dt><dd>{tenant.customer_type === "company" ? t("Company") : t("Person")}</dd>{profileFields.map(([field, label]) => <div key={field}><dt>{t(label)}</dt><dd>{tenant[field] || "—"}</dd></div>)}</dl>}
    </section><section className="auth-panel"><h2>{t("Your websites")}</h2><p className="muted">{t("Website permissions are assigned separately by your Webdock operator.")}</p>
      {sites.length ? sites.map((site) => <article className="access-record" key={site.id}><h3><a href={site.url}>{site.name}</a></h3><p>{authLabel(site.role, t)}</p></article>) : <p>{t("No websites are currently authorized for your account in this tenant.")}</p>}
      {operator && <p><Link href="/people">{t("Manage website permissions")}</Link></p>}
    </section></div>
    {canManage && <>
      <section className="account-section"><h2>{t("Invite a person")}</h2><p>{t("New people receive a secure link to choose their password. Tenant access starts after they accept the invitation.")}</p>
        <TenantForm customer={customerID} action="invite" label={t("Send invitation")}><label className="field">{t("Name")}<input name="name" required maxLength={160} /></label><label className="field">{t("Email")}<input name="email" type="email" required maxLength={254} /></label><TenantRole value={operator && members.length === 0 ? "admin" : "member"} /></TenantForm>
      </section>
      <section className="account-section"><h2>{t("People")}</h2><p className="muted">{t("Administrators can edit this profile and invite people. Existing role changes and removals are managed by your Webdock operator.")}</p><div className="access-grid">
        {members.map((member) => <article className="access-record" key={member.id}><h3>{member.name}</h3><p>{member.email}<br />{authLabel(member.role, t)} · {member.banned ? t("Suspended") : !member.emailVerified || member.mustChangePassword ? t("Setup pending") : t("Active")}</p>
          {operator && <><TenantForm customer={customerID} action="role" label={t("Update tenant role")}><input type="hidden" name="member" value={member.id} /><TenantRole value={member.role} /></TenantForm><TenantForm customer={customerID} action="remove" label={t("Remove membership")} confirm={t("Remove this person’s tenant and website access")}><input type="hidden" name="member" value={member.id} /></TenantForm></>}
        </article>)}
      </div>{!members.length && <p>{t("No customer members yet. Invite the first person with the Administrator role.")}</p>}</section>
      <section className="account-section"><h2>{t("Invitations")}</h2>{invitations.map((invite) => <article className="access-record" key={invite.id}><h3>{invite.email}</h3><p>{authLabel(invite.role, t)} · {invite.status === "pending" && new Date(invite.expiresAt) < new Date() ? t("Expired") : authLabel(invite.status, t)}</p>
        {invite.status === "pending" && <><TenantForm customer={customerID} action="resend" label={t("Resend invitation")}><input type="hidden" name="invitation" value={invite.id} /></TenantForm><TenantForm customer={customerID} action="cancel" label={t("Cancel invitation")} confirm={t("Cancel the invitation and pending website access")}><input type="hidden" name="invitation" value={invite.id} /></TenantForm></>}
      </article>)}{!invitations.length && <p>{t("No invitations yet.")}</p>}</section>
    </>}
  </div>;
}
