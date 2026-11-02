import {hasProductModules,optionalModuleData} from "@/lib/module-data";
import { AreaNavigation } from "@/components/area-navigation";
import {uiLabel} from "@/lib/ui-labels";

import { getRequestI18n } from '@webdock/i18n/next';
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
export async function generateMetadata(){ const i18n = await getRequestI18n(); return { title: i18n.t("Plan and usage") }; }
const bytes = (value: number | null, locale: string, unavailable: string) =>
  value === null
    ? unavailable
    : `${(value / 1_000_000).toLocaleString(locale === "de" ? "de-DE" : "en-GB", { maximumFractionDigits: 3 })} MB`;
export default async function Usage({
  params, searchParams,
}: {
  params: Promise<{ customerID: string }>; searchParams:Promise<{view?:string}>;
}) {
  const i18n = await getRequestI18n();

  const { customerID } = await params,
    requestHeaders = await headers();
  const session = await auth.api.getSession({ headers: requestHeaders });
  if (!session)
    redirect(
      `/api/sso/login?returnTo=${encodeURIComponent(`/tenants/${customerID}/usage`)}`,
    );
  if (!hasProductModules(session.installation)) notFound();
  let tenantData, data, storage, plans, mail;
  try {
    [tenantData, data, storage, plans, mail] = await Promise.all([
      getTenant(requestHeaders, customerID),
      getTenantPlan(requestHeaders, customerID),
      optionalModuleData(session.installation,"storage",()=>getTenantStorage(requestHeaders, customerID)),
      session.operator && !session.preview ? getPlans(requestHeaders) : Promise.resolve([]),
      optionalModuleData(session.installation,"mail",()=>getTenantMail(requestHeaders, customerID)),
    ]);
  } catch (error) {
    if (
      error instanceof AccessError ||
      error instanceof PlanError ||
      error instanceof StorageUsageError
    )
      notFound();
    throw error;
  }
  // This request-bound Server Component compares stored snapshots with the server clock.
  // eslint-disable-next-line react-hooks/purity
  const now = Date.now();
  const query = await searchParams;
  const view = tenantData.operator && (query.view === "plan" || (query.view === "sources" && storage)) ? query.view : "overview";
  const base = `/tenants/${customerID}/usage`;
  const storageLimit = data.subscription?.effective.storageBytes;
  return (
    <div className="access-page">
      <header className="account-heading">
        <div>
          <h1>{i18n.t("Plan and usage")}</h1>
          <p className="muted">{tenantData.tenant.name}</p>
        </div>
      </header>
      <AreaNavigation links={[{href:base+"?view=overview",label:i18n.t("Usage"),exact:true},...(tenantData.operator ? [{href:base+"?view=plan",label:i18n.t("Plan & offers"),exact:true},...(storage ? [{href:base+"?view=sources",label:i18n.t("Measurement sources"),exact:true}] : [])] : [])]}/>
      {view === "overview" && <div className="access-grid">
        <section className="auth-panel">
          <h2>{data.subscription?.name || i18n.t("No plan assigned")}</h2>
          {data.subscription ? (
            <>
              <p>{data.subscription.description}</p>
              <h3>{i18n.t("Effective allowances")}</h3>
              <AllowanceList values={data.subscription.effective} />
              <details>
                <summary>{i18n.t("Base plan and extras")}</summary>
                <h3>{i18n.t("Base plan")}</h3>
                <AllowanceList values={data.subscription.base} />
                <h3>{i18n.t("Extras")}</h3>
                <AllowanceList values={data.subscription.extras} />
              </details>
            </>
          ) : (
            <p>{i18n.t("Your operator can assign a plan or send you an individual offer.")}</p>
          )}
          <p className="help">{i18n.t("These are configured allocations. Storage and transfer limits are not yet enforced. Mail sending is governed by the provider’s actual quota.")}</p>
        </section>
        {(storage || mail) && <section className="auth-panel">
          {storage && <>
          <h2>{i18n.t("Measured storage")}</h2>
          <dl>
            <dt>{i18n.t("Production")}</dt>
            <dd>{bytes(storage.productionBytes, i18n.locale, i18n.t("Unavailable"))}</dd>
            <dt>{i18n.t("Preview")}</dt>
            <dd>{bytes(storage.previewBytes, i18n.locale, i18n.t("Unavailable"))}</dd>
          </dl>
          {storage.productionBytes !== null && storageLimit != null && (
            <>
              <p>{i18n.t("Production: ")}{bytes(storage.productionBytes, i18n.locale, i18n.t("Unavailable"))}{i18n.t(" of")}{" "}
                {bytes(storageLimit, i18n.locale, i18n.t("Unavailable"))}{i18n.t(" configured")}</p>
              {storageLimit > 0 && (
                <progress
                  aria-label={i18n.t("Production storage against configured allowance")}
                  value={storage.productionBytes}
                  max={storageLimit}
                />
              )}
              {storage.productionBytes > storageLimit && (
                <p className="notice">{i18n.t("Production storage exceeds the configured allocation.")}</p>
              )}
            </>
          )}
          <p className="help">{i18n.t("Snapshots of connected stores. Missing or failed measurements are unavailable, never counted as zero. Preview storage is shown separately.")}</p>
          </>}
          {mail && <><h3>{i18n.t("Mail provider usage")}</h3>
          <p>
            {mail.account?.sent ?? i18n.t("Unavailable")}{i18n.t(" sent · Provider limit:")}{" "}
            {mail.account?.limit ?? i18n.t("Unavailable")}
          </p>
          {mail.account?.sent != null &&
            mail.account.limit != null &&
            mail.account.limit > 0 && (
              <progress
                aria-label={i18n.t("Mail sent against provider quota")}
                value={mail.account.sent}
                max={mail.account.limit}
              />
            )}
          <p className="help">{i18n.t("Period: ")}{mail.account?.interval || i18n.t("Not reported")}
            <br />{i18n.t("Last checked:")}{" "}
            {mail.account?.checkedAt
              ? `${new Date(mail.account.checkedAt).toLocaleString(i18n.locale === "de" ? "de-DE" : "en-GB", { timeZone: "UTC" })} UTC`
              : i18n.t("Not checked")}
          </p>
          {mail.account?.checkedAt &&
            now - new Date(mail.account.checkedAt).getTime() > 86_400_000 && (
              <p className="notice">{i18n.t("Mail usage is more than 24 hours old.")}</p>
            )}
          <p>
            <Link href={`/tenants/${customerID}/mail`}>{i18n.t("View Mail usage and provider quota")}</Link>
          </p>
          </>}
          <p className="muted">{i18n.t("Transfer usage is not currently measured here.")}</p>
        </section>}
      </div>}
      {view === "sources" && storage && <section className="account-section">
        <h2>{i18n.t("Storage measurements")}</h2>
        {!storage.stores.length && (
          <p>{i18n.t("No storage connections yet. Usage is unavailable until a store is connected and measured.")}</p>
        )}
        <div className="access-grid">
          {storage.stores.map((store) => (
            <article className="access-record" key={store.id}>
              <h3>{store.label}</h3>
              <p>
                {store.environment === "production" ? i18n.t("Production") : i18n.t("Preview")}
              </p>
              <dl>
                <dt>{i18n.t("Stored data")}</dt>
                <dd>{bytes(store.bytes, i18n.locale, i18n.t("Unavailable"))}</dd>
                <dt>{i18n.t("Objects")}</dt>
                <dd>{store.objects ?? i18n.t("Unavailable")}</dd>
                <dt>{i18n.t("Last checked")}</dt>
                <dd>
                  {store.checkedAt
                    ? `${new Date(store.checkedAt).toLocaleString(i18n.locale === "de" ? "de-DE" : "en-GB", { timeZone: "UTC" })} UTC`
                    : i18n.t("Not checked")}
                </dd>
              </dl>
              {store.error && <p className="notice error">{i18n.error(store.error)}</p>}
              {store.checkedAt &&
                now - new Date(store.checkedAt).getTime() > 86_400_000 && (
                  <p className="notice">{i18n.t("This snapshot is more than 24 hours old. Refresh it before relying on these figures.")}</p>
                )}
            </article>
          ))}
        </div>
      </section>}
      {!tenantData.operator && (
        <section className="account-section">
          <h2>{i18n.t("Offers")}</h2>
          {data.offers.length ? (
            data.offers.map((offer) => (
              <article className="access-record" key={offer.id}>
                <h3>{offer.name}</h3>
                <p>
                  {i18n.t(uiLabel(offer.status))}{i18n.t(" · Expires")}{" "}
                  {new Date(offer.expiresAt).toLocaleString(i18n.locale === "de" ? "de-DE" : "en-GB", {
                    timeZone: "UTC",
                  })}{" "}{i18n.t("UTC")}</p>
              </article>
            ))
          ) : (
            <p>{i18n.t("No offers yet.")}</p>
          )}
        </section>
      )}
      {tenantData.operator && view !== "overview" && (
        <>
          <p>
            <Link href="/admin/plans">{i18n.t("Manage reusable plans")}</Link>
          </p>
          {tenantData.tenant.status === "active" ? (
            <UsageForms key={customerID+":"+view}
              customerID={customerID}
              plans={plans}
              data={data}
              storage={storage ?? {stores:[],productionBytes:null,previewBytes:null}}
              mode={view === "sources" ? "storage" : "plan"}
            />
          ) : (
            <p className="notice">{i18n.t("This tenant is archived. Changes are disabled.")}</p>
          )}
        </>
      )}
    </div>
  );
}
