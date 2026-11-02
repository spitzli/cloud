import {getStudioSession} from "@/lib/studio-client";
import {hasProductModules} from "@/lib/module-data";

import { getRequestI18n } from '@webdock/i18n/next';
import { headers } from "next/headers";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";
import { AccessError } from "@/lib/access-management";
import { auth } from "@/lib/auth";
import { getPlans, PlanError } from "@/lib/plans";
import {
  AllowanceList,
  PlanFields,
  PlanIdentity,
} from "@/components/plan-fields";
import { PlanForm } from "./form";
export async function generateMetadata(){ const i18n = await getRequestI18n(); return { title: i18n.t("Plans") }; }
export default async function Plans() {
  if (!hasProductModules((await getStudioSession())?.installation)) notFound();
  const i18n = await getRequestI18n();

  const requestHeaders = await headers();
  if (!(await auth.api.getSession({ headers: requestHeaders })))
    redirect("/api/sso/login?returnTo=%2Fadmin%2Fplans");
  let plans;
  try {
    plans = await getPlans(requestHeaders);
  } catch (error) {
    if (error instanceof AccessError || error instanceof PlanError) notFound();
    throw error;
  }
  return (
    <div className="access-page">
      <header className="account-heading">
        <div>
          <h1>{i18n.t("Plans")}</h1>
          <p className="muted">{i18n.t("Reusable allocations for your customers. Assign a plan or prepare an individual offer from a tenant’s usage page.")}</p>
        </div>
        <Link className="button secondary" href="/admin">{i18n.t("Administration")}</Link>
      </header>
      <details className="auth-panel" open={plans.length === 0}>
        <summary>{i18n.t("Create a reusable plan")}</summary>
        <PlanForm action="create-plan" label={i18n.t("Create plan")}>
          <PlanIdentity />
          <PlanFields />
        </PlanForm>
      </details>
      <section className="account-section">
        <h2>{i18n.t("Plan library")}</h2>
        <p>
          <Link href="/tenants">{i18n.t("Choose a tenant to assign a plan")}</Link>
        </p>
        <div className="access-grid">
          {plans.map((plan) => (
            <article className="access-record" key={plan.id}>
              <h3>{plan.name}</h3>
              <p>{plan.description}</p>
              <AllowanceList values={plan.allowances} />
            </article>
          ))}
        </div>
        {!plans.length && <p>{i18n.t("No plans yet. Create your first plan above.")}</p>}
      </section>
    </div>
  );
}
