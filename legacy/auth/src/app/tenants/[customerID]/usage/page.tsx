import { authLabel } from "@/lib/i18n-labels";
import { getRequestI18n } from "@webdock/i18n/next";
import { headers } from "next/headers";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";
import { auth } from "@/lib/auth";
import { AccessError } from "@/lib/access-management";
import { getTenantMail } from "@/lib/tenant-mail";
import { getTenant } from "@/lib/tenants";
import { getPlans, getTenantPlan, PlanError } from "@/lib/plans";
import { getTenantStorage, StorageUsageError } from "@/lib/storage-usage";
import { AllowanceList } from "@/components/plan-fields";
import { UsageForms } from "./form";
export async function generateMetadata() { const { t } = await getRequestI18n(); return { title: t("Plan and usage") }; }
export default async function Usage({ params }: { params: Promise<{ customerID: string }> }) {
 const { t, error: translateError, date: formatDate, number: formatNumber } = await getRequestI18n();
  const bytes = (value: number | null) => value === null ? t("Unavailable") : t("{value1} MB", {value1: formatNumber(value / 1_000_000, { maximumFractionDigits: 3 })});
  const { customerID } = await params, requestHeaders = await headers();
  if (!(await auth.api.getSession({ headers: requestHeaders }))) redirect("/sign-in");
  let tenantData, data, storage, plans, mail;
  try {
    tenantData = await getTenant(requestHeaders, customerID);
    [data, storage, plans, mail] = await Promise.all([getTenantPlan(requestHeaders, customerID), getTenantStorage(requestHeaders, customerID), tenantData.operator ? getPlans(requestHeaders) : Promise.resolve([]), getTenantMail(requestHeaders, customerID)]);
  } catch (error) { if (error instanceof AccessError || error instanceof PlanError || error instanceof StorageUsageError) notFound(); throw error; }
  // This request-bound Server Component compares stored snapshots with the server clock.
  // eslint-disable-next-line react-hooks/purity
  const now = Date.now();
  const storageLimit = data.subscription?.effective.storageBytes;
  return <div className="access-page"><header className="account-heading"><div><h1>{t("Plan and usage")}</h1><p className="muted">{tenantData.tenant.name}</p></div><Link className="button secondary" href={`/tenants/${customerID}`}>{t("Back to tenant")}</Link></header>
    <div className="access-grid"><section className="auth-panel"><h2>{data.subscription?.name || t("No plan assigned")}</h2>{data.subscription ? <><p>{data.subscription.description}</p><h3>{t("Effective allowances")}</h3><AllowanceList values={data.subscription.effective} /><details><summary>{t("Base plan and extras")}</summary><h3>{t("Base plan")}</h3><AllowanceList values={data.subscription.base} /><h3>{t("Extras")}</h3><AllowanceList values={data.subscription.extras} /></details></> : <p>{t("Your operator can assign a plan or send you an individual offer.")}</p>}<p className="help">{t("These are configured allocations. Storage and transfer limits are not yet enforced. Mail sending is governed by the provider’s actual quota.")}</p></section>
    <section className="auth-panel"><h2>{t("Measured storage")}</h2><dl><dt>{t("Production")}</dt><dd>{bytes(storage.productionBytes)}</dd><dt>{t("Preview")}</dt><dd>{bytes(storage.previewBytes)}</dd></dl>{storage.productionBytes !== null && storageLimit != null && <><p>{t("Production: ")}{bytes(storage.productionBytes)} {t(" of ")}{bytes(storageLimit)} {t(" configured")}</p>{storageLimit > 0 && <progress aria-label={t("Production storage against configured allowance")} value={storage.productionBytes} max={storageLimit} />}{storage.productionBytes > storageLimit && <p className="notice">{t("Production storage exceeds the configured allocation.")}</p>}</>}<p className="help">{t("Snapshots of connected stores. Missing or failed measurements are unavailable, never counted as zero. Preview storage is shown separately.")}</p><h3>{t("Mail provider usage")}</h3><p>{mail.account?.sent == null ? t("Unavailable") : formatNumber(mail.account.sent)} {t(" sent · Provider limit: ")}{mail.account?.limit == null ? t("Unavailable") : formatNumber(mail.account.limit)}</p>{mail.account?.sent != null && mail.account.limit != null && mail.account.limit > 0 && <progress aria-label={t("Mail sent against provider quota")} value={mail.account.sent} max={mail.account.limit} />}<p className="help">{t("Period: ")}{authLabel(mail.account?.interval,t) || t("Not reported")}<br />{t("Last checked: ")}{mail.account?.checkedAt ? t("{value1} UTC", {value1: formatDate(mail.account.checkedAt, { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" })}) : t("Not checked")}</p>{mail.account?.checkedAt && now - new Date(mail.account.checkedAt).getTime() > 86_400_000 && <p className="notice">{t("Mail usage is more than 24 hours old.")}</p>}<p><Link href={`/tenants/${customerID}/mail`}>{t("View Mail usage and provider quota")}</Link></p><p className="muted">{t("Transfer usage is not currently measured here.")}</p></section></div>
    <section className="account-section"><h2>{t("Storage measurements")}</h2>{!storage.stores.length && <p>{t("No storage connections yet. Usage is unavailable until a store is connected and measured.")}</p>}<div className="access-grid">{storage.stores.map(store => <article className="access-record" key={store.id}><h3>{store.label}</h3><p>{store.environment === "production" ? t("Production") : t("Preview")}</p><dl><dt>{t("Stored data")}</dt><dd>{bytes(store.bytes)}</dd><dt>{t("Objects")}</dt><dd>{store.objects == null ? t("Unavailable") : formatNumber(store.objects)}</dd><dt>{t("Last checked")}</dt><dd>{store.checkedAt ? t("{value1} UTC", {value1: formatDate(store.checkedAt, { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" })}) : t("Not checked")}</dd></dl>{store.error && <p className="notice error">{translateError(store.error)}</p>}{store.checkedAt && now - new Date(store.checkedAt).getTime() > 86_400_000 && <p className="notice">{t("This snapshot is more than 24 hours old. Refresh it before relying on these figures.")}</p>}</article>)}</div></section>
    {!tenantData.operator && <section className="account-section"><h2>{t("Offers")}</h2>{data.offers.length ? data.offers.map(offer => <article className="access-record" key={offer.id}><h3>{offer.name}</h3><p>{authLabel(offer.status, t)} {t(" · Expires ")}{formatDate(offer.expiresAt, { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" })} {t(" UTC")}</p></article>) : <p>{t("No offers yet.")}</p>}</section>}
    {tenantData.operator && <><p><Link href="/admin/plans">{t("Manage reusable plans")}</Link></p>{tenantData.tenant.status === "active" ? <UsageForms customerID={customerID} plans={plans} data={data} storage={storage} /> : <p className="notice">{t("This tenant is archived. Changes are disabled.")}</p>}</>}
  </div>;
}
