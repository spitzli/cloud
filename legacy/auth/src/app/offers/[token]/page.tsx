import { authLabel } from "@/lib/i18n-labels";
import { getRequestI18n } from "@webdock/i18n/next";
import type { Metadata } from "next";
import { headers } from "next/headers";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";
import { auth } from "@/lib/auth";
import { AccessError } from "@/lib/access-management";
import { getOffer, PlanError } from "@/lib/plans";
import { AllowanceList } from "@/components/plan-fields";
import { OfferForm } from "./form";
export async function generateMetadata(): Promise<Metadata> { const { t } = await getRequestI18n(); return { title: t("Your offer"), robots: { index: false, follow: false }, referrer: "same-origin" }; }
export default async function Offer({ params }: { params: Promise<{ token: string }> }) {
 const { t, date: formatDate } = await getRequestI18n();
  const { token } = await params, requestHeaders = await headers();
  if (!/^[A-Za-z0-9_-]{43}$/.test(token)) notFound();
  if (!(await auth.api.getSession({ headers: requestHeaders }))) redirect(`/sign-in?offer=${encodeURIComponent(token)}`);
  let offer;
  try { offer = await getOffer(requestHeaders, token); } catch (error) { if (error instanceof PlanError || error instanceof AccessError) notFound(); throw error; }
  return <div className="access-page"><header className="account-heading"><div><p className="eyebrow">{t("Offer for ")}{offer.customerName}</p><h1>{offer.name}</h1><p>{offer.description}</p></div><Link className="button secondary" href={`/tenants/${offer.customerID}/usage`}>{t("Tenant plan")}</Link></header>
    <p className="notice">{authLabel(offer.status, t)} {t(" · Expires ")}{formatDate(offer.expiresAt, { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" })} {t(" UTC")}</p>
    <div className="access-grid"><section className="auth-panel"><h2>{t("Offered plan")}</h2><AllowanceList values={offer.base} /></section><section className="auth-panel"><h2>{t("Preserved extras")}</h2><AllowanceList values={offer.extras} /></section><section className="auth-panel"><h2>{t("Effective allowances")}</h2><AllowanceList values={offer.effective} /></section></div>
    <section className="account-section"><h2>{t("Commercial terms")}</h2><p style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{offer.terms || t("No additional commercial terms specified.")}</p><p className="help">{t("Acceptance assigns the displayed plan and preserves the displayed extras. This page does not collect payment. Storage and transfer limits are configured allocations and are not yet enforced; Mail sending uses the provider quota.")}</p>
      {offer.canAccept ? <OfferForm token={token} /> : <p className="notice">{offer.status === "accepted" ? t("Offer accepted. The agreed allocation was assigned to your tenant. No payment was collected here.") : offer.status === "pending" ? t("Only an authorized administrator of this tenant can accept an active offer. If the plan has changed, ask your operator for a new offer.") : t("This offer is no longer available for acceptance.")}</p>}
    </section>
  </div>;
}
