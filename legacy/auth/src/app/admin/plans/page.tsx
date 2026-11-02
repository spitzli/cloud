import { getRequestI18n } from "@webdock/i18n/next";
import { headers } from "next/headers";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";
import { AccessError } from "@/lib/access-management";
import { auth } from "@/lib/auth";
import { getPlans, PlanError } from "@/lib/plans";
import { AllowanceList, PlanFields, PlanIdentity } from "@/components/plan-fields";
import { PlanForm } from "./form";
export async function generateMetadata() { const { t } = await getRequestI18n(); return { title: t("Plans") }; }
export default async function Plans() {
 const { t } = await getRequestI18n();
  const requestHeaders = await headers();
  if (!(await auth.api.getSession({ headers: requestHeaders }))) redirect("/sign-in");
  let plans;
  try { plans = await getPlans(requestHeaders); } catch (error) { if (error instanceof AccessError || error instanceof PlanError) notFound(); throw error; }
  return <div className="access-page"><header className="account-heading"><div><p className="eyebrow">{t("Root admin")}</p><h1>{t("Plans")}</h1><p className="muted">{t("Reusable allocations for your customers. Assign a plan or prepare an individual offer from a tenant’s usage page.")}</p></div><Link className="button secondary" href="/admin">{t("Administration")}</Link></header>
    <section className="auth-panel"><h2>{t("Create a reusable plan")}</h2><PlanForm action="create-plan" label={t("Create plan")}><PlanIdentity /><PlanFields /></PlanForm></section>
    <section className="account-section"><h2>{t("Plan library")}</h2><p><Link href="/tenants">{t("Choose a tenant to assign a plan")}</Link></p><div className="access-grid">{plans.map(plan => <article className="access-record" key={plan.id}><h3>{plan.name}</h3><p>{plan.description}</p><AllowanceList values={plan.allowances} /></article>)}</div>{!plans.length && <p>{t("No plans yet. Create your first plan above.")}</p>}</section>
  </div>;
}
