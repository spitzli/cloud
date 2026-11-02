"use client";
import { msgid } from "@webdock/i18n";
import { authLabel } from "@/lib/i18n-labels";
import { useI18n } from "@webdock/i18n/react";
import { useActionState, useState, type ReactNode } from "react";
import type { getPlans, getTenantPlan } from "@/lib/plans";
import type { getTenantStorage } from "@/lib/storage-usage";
import { AllowanceList, PlanFields, PlanIdentity } from "@/components/plan-fields";
import { usageAction } from "./actions";

export function UsageForms({ customerID, plans, data, storage }: {
  customerID: string; plans: Awaited<ReturnType<typeof getPlans>>;
  data: Awaited<ReturnType<typeof getTenantPlan>>; storage: Awaited<ReturnType<typeof getTenantStorage>>;
}) {
 const { t, error: translateError, date: formatDate } = useI18n();
  const [state, submit, pending] = useActionState(usageAction, {});
  const [selected, setSelected] = useState("");
  const [copied, setCopied] = useState(false);
  const plan = plans.find(item => item.id === selected);
  function form(action: string, label: string, children: ReactNode) {
    return <form action={submit} className="access-form" aria-busy={pending} onSubmit={() => setCopied(false)}>
      <input type="hidden" name="action" value={action} /><input type="hidden" name="customerID" value={customerID} /><input type="hidden" name="revision" value={data.revision} />
      {children}<button className="button secondary" disabled={pending}>{pending ? t("Working…") : t(label)}</button>
    </form>;
  }
  return <>
    <section className="account-section" aria-label={t("Change result")} aria-live="polite">
      {!pending && state.error && <p className="notice error" role="alert">{translateError(state.error)}</p>}
      {!pending && state.message && <p className="notice" role="status">{t(state.message)}</p>}
      {!pending && state.offerURL && <div className="notice"><label className="field">{t("Shareable offer link")}<input readOnly value={state.offerURL} onFocus={event => event.currentTarget.select()} /></label><button type="button" className="button secondary" onClick={async () => { try { await navigator.clipboard.writeText(state.offerURL!); setCopied(true); } catch { setCopied(false); } }}>{copied ? t("Copied") : t("Copy link")}</button><p>{t("Copy this link now. It is shown only after creation and disappears after your next action. Share it with this tenant’s administrator.")}</p></div>}
    </section>
    <div className="access-grid"><section className="auth-panel"><h2>{t("Assign a plan")}</h2><p>{t("Applies immediately. Existing extras are preserved.")}</p>
      {plans.length ? form("assign", "Assign plan", <label className="field">{t("Plan")}<select name="planID" required defaultValue=""><option value="" disabled>{t("Choose a plan")}</option>{plans.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select></label>) : <p>{t("Create a reusable plan in the plan library first.")}</p>}
    </section><section className="auth-panel"><h2>{t("Additional allowances")}</h2><p>{t("Save the total extras for this tenant. This replaces the previous extras.")}</p>{data.subscription ? form("extras", "Save extras", <PlanFields key={data.revision} values={data.subscription.extras} extras />) : <p>{t("Assign a plan before adding extras.")}</p>}</section></div>
    <section className="account-section"><h2>{t("Individual plan for this customer")}</h2><p>{t("Applies immediately to this customer only and replaces their current plan. It is not added to the reusable plan library. Existing extras are preserved.")}</p>
      {form("assign-custom", msgid("Assign individual plan"), <div key={data.revision}><PlanIdentity name={data.subscription?.name} description={data.subscription?.description} /><PlanFields values={data.subscription?.base} /></div>)}
    </section>
    <section className="account-section"><h2>{t("Create an individual offer")}</h2><p>{t("Prepare adjusted plan allowances and optional commercial terms for the customer to accept. Acceptance assigns these allowances and preserves the extras shown below. This does not collect payment.")}</p>
      {form("create-offer", "Create offer link", <>
        <label className="field">{t("Start from a plan")}<select name="planID" value={selected} onChange={event => setSelected(event.target.value)}><option value="">{t("Custom offer")}</option>{plans.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select></label>
        <div key={selected}><PlanIdentity name={plan?.name} description={plan?.description} /><PlanFields values={plan?.allowances} /></div>
        <label className="field">{t("Commercial terms (optional)")}<textarea name="terms" maxLength={8000} placeholder={t("Agreed price, service scope, or other terms")} /></label>
        <label className="field">{t("Expires after (days)")}<input name="expiresDays" type="number" min={1} max={90} defaultValue={14} required /></label>
        {data.subscription && <details><summary>{t("Extras preserved on acceptance")}</summary><AllowanceList values={data.subscription.extras} /></details>}
      </>)}
    </section>
    <section className="account-section"><h2>{t("Offers")}</h2>{data.offers.length ? data.offers.map(offer => <article className="access-record" key={offer.id}><h3>{offer.name}</h3><p>{authLabel(offer.status, t)} {t(" · Expires ")}{formatDate(offer.expiresAt, { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" })} {t(" UTC")}</p>{offer.status === "pending" && form("revoke-offer", "Revoke offer", <input type="hidden" name="offerID" value={offer.id} />)}</article>) : <p>{t("No offers yet.")}</p>}</section>
    <section className="account-section"><h2>{t("Storage connections")}</h2><p>{t("Connect each Blob store or a tenant’s exclusive path prefix. Credentials are saved privately and are never displayed. Production and preview storage are measured separately.")}</p>
      {form("storage-add", "Connect storage", <>
        <label className="field">{t("Label")}<input name="label" required maxLength={160} /></label>
        <label className="field">{t("Environment")}<select name="environment"><option value="production">{t("Production")}</option><option value="preview">{t("Preview")}</option></select></label>
        <label className="field">{t("Blob store ID")}<input name="storeID" required maxLength={160} /></label>
        <label className="field">{t("Path prefix (optional)")}<input name="prefix" maxLength={1024} /><span className="help">{t("Blank measures the entire store. Use an exclusive prefix for a shared store.")}</span></label>
        <label className="field">{t("Blob read/write token")}<input name="token" type="password" autoComplete="new-password" required /></label>
      </>)}
      {storage.stores.map(store => <article className="access-record" key={store.id}><h3>{store.label}</h3><p>{authLabel(store.environment, t)} · {store.storeID} · {store.prefix || t("Whole store")}</p>{form("storage-refresh", "Refresh measurement", <input type="hidden" name="storageID" value={store.id} />)}{form("storage-remove", "Disconnect storage", <><input type="hidden" name="storageID" value={store.id} /><label className="check"><input type="checkbox" required />{t("Remove this measurement connection. Stored files remain unchanged.")}</label></>)}</article>)}
    </section>
  </>;
}
