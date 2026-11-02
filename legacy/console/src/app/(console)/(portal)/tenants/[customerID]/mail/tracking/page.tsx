import {mailRead,type MailConnection} from '@/lib/mail-product';import type {TrackingView} from '@/lib/studio-contracts';
import {msgid} from '@webdock/i18n';

import { getRequestI18n } from '@webdock/i18n/next';
import { headers } from "next/headers";
import { notFound, redirect } from "next/navigation";
import { auth } from "@/lib/auth";
import { AccessError } from "@/lib/access-management";
import { getTenantTracking } from "@/lib/mail-tracking";
import { TrackingForm } from "./form";
export async function generateMetadata(){ const i18n = await getRequestI18n(); return { title: i18n.t("Mail tracking") }; }
export default async function Tracking({
  params,searchParams,
}: {
  params: Promise<{ customerID: string }>;searchParams:Promise<Record<string,string|string[]|undefined>>;
}) {
  const i18n = await getRequestI18n();

  const { customerID } = await params,
    requestHeaders = await headers();
  if (!(await auth.api.getSession({ headers: requestHeaders })))
    redirect(
      `/api/sso/login?returnTo=${encodeURIComponent(`/tenants/${customerID}/mail/tracking`)}`,
    );
  const query=await searchParams;if(query.connection!==undefined&&typeof query.connection!=='string')notFound();const connectionID=query.connection??(await mailRead<MailConnection[]>('mailConnections',customerID))[0]?.id;
  let data;
  try {
    data = connectionID?await mailRead<TrackingView>('mailTracking',customerID,connectionID):await getTenantTracking(requestHeaders, customerID);
  } catch (error) {
    if (error instanceof AccessError) notFound();
    throw error;
  }
  const editable = data.canManage && !data.unavailable;
  return (
    <div className="access-page">
      <header className="account-heading">
        <div>
          <h1>{i18n.t("Mail tracking")}</h1>
          <p>
            {data.tenant.name}{i18n.t(" · Track opens and clicks with your own domain.")}</p>
        </div>
      </header>
      {data.unavailable && (
        <p className="notice">
          {i18n.error(data.unavailable)}{i18n.t(" Contact your Webdock operator to complete setup.")}</p>
      )}
      <section className="auth-panel">
        <h2>{i18n.t("Tracking settings")}</h2>
        <p className="muted">{i18n.t("Values show the last successful provider check. No email is sent from this screen.")}</p>
        {data.checkedAt && (
          <p>{i18n.t("Last checked:")}{" "}
            {new Date(data.checkedAt).toLocaleString(i18n.locale === "de" ? "de-DE" : "en-GB", {
              timeZone: "UTC",
            })}{" "}{i18n.t("UTC")}</p>
        )}
        {editable && (
          <TrackingForm
            customer={customerID} connection={connectionID}
            action="refresh"
            label={i18n.t("Refresh tracking status")}
          />
        )}
        <div className="access-grid">
          {(
            [
              [
                "opening",
                msgid("Open tracking"),
                data.settings?.opening.enabled,
                data.settings?.opening.forced,
              ],
              [
                "click",
                msgid("Click tracking"),
                data.settings?.click.enabled,
                data.settings?.click.forced,
              ],
              [
                "custom",
                "Custom tracking domains",
                data.settings?.custom,
                false,
              ],
              [
                "matchSender",
                msgid("Match sender domain"),
                data.settings?.info.match_sender,
                false,
              ],
            ] as const
          ).map(([setting, label, enabled, forced]) => (
            <article className="access-record" key={setting}>
              <h3>{i18n.t(label)}</h3>
              <p>
                {enabled == null
                  ? i18n.t("Status unavailable — refresh to check")
                  : enabled
                    ? i18n.t("Enabled")
                    : i18n.t("Disabled")}
                {forced ? i18n.t(" · Required by account policy") : ""}
              </p>
              {editable && !forced && (
                <TrackingForm
                  customer={customerID} connection={connectionID}
                  action="setting"
                  label={i18n.t("Update {setting}",{setting:i18n.t(label)})}
                >
                  <input type="hidden" name="setting" value={setting} />
                  <label className="field">{i18n.t("Choose state")}<select name="value" defaultValue="" required>
                      <option value="" disabled>{i18n.t("Select a state")}</option>
                      <option value="yes">{i18n.t("Enabled")}</option>
                      <option value="no">{i18n.t("Disabled")}</option>
                    </select>
                  </label>
                </TrackingForm>
              )}
            </article>
          ))}
        </div>
        {data.settings && (
          <p className="muted">
            {data.settings.info.all_domains_disabled
              ? i18n.t("All custom domains are disabled.")
              : i18n.t("At least one custom domain is enabled.")}{" "}
            {data.settings.info.no_default_domain
              ? i18n.t("No default tracking domain is selected.")
              : i18n.t("A default tracking domain is selected.")}
          </p>
        )}
      </section>
      <section className="account-section">
        <h2>{i18n.t("Custom tracking domains")}</h2>
        <p>{i18n.t("Use a subdomain of a verified sender domain. Creating it does not enable open or click tracking.")}</p>
        {editable &&
          (data.senders.length ? (
            <TrackingForm
              customer={customerID} connection={connectionID}
              action="create"
              label={i18n.t("Create tracking domain")}
            >
              <label className="field">{i18n.t("Sender domain")}<select name="senderDomainID" required>
                  {data.senders.map((sender) => (
                    <option key={sender.id} value={sender.id}>
                      {sender.domain}
                    </option>
                  ))}
                </select>
              </label>
              <label className="field">{i18n.t("Subdomain prefix")}<input
                  name="prefix"
                  defaultValue="links"
                  required
                  maxLength={63}
                  pattern="[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?"
                />
              </label>
              <p className="muted">{i18n.t("For example, links with example.com creates links.example.com.")}</p>
            </TrackingForm>
          ) : (
            <p>{i18n.t("Add and verify a sender domain in Mail before creating a tracking domain.")}</p>
          ))}
        <div className="access-grid">
          {data.domains.map((domain) => (
            <article className="access-record" key={domain.id}>
              <h3>{domain.domain}</h3>
              {!domain.mapped ? (
                <>
                  <p className="notice">{i18n.t("Creation needs reconciliation. Refresh to find the reserved domain; do not create another copy.")}</p>
                  {editable &&
                    (domain.cancellable ? (
                      <TrackingForm
                        customer={customerID} connection={connectionID}
                        action="cancel-reservation"
                        label={i18n.t("Cancel unused reservation")}
                        confirm={i18n.t("Cancel this reservation only if the provider confirms no matching tracking domain exists.")}
                      >
                        <input
                          type="hidden"
                          name="domainID"
                          value={domain.id}
                        />
                      </TrackingForm>
                    ) : (
                      <p className="muted">{i18n.t("An unused reservation can be cancelled after 15 minutes, once a fresh provider check confirms that no domain was created.")}</p>
                    ))}
                </>
              ) : (
                <>
                  <p>
                    {domain.snapshot.verified === undefined
                      ? i18n.t("Status not checked")
                      : domain.snapshot.verified
                        ? i18n.t("Verified")
                        : i18n.t("DNS verification pending")}{" "}
                    ·{" "}
                    {domain.snapshot.ssl === undefined
                      ? i18n.t("HTTPS not checked")
                      : domain.snapshot.ssl
                        ? i18n.t("HTTPS ready")
                        : i18n.t("HTTPS pending")}
                  </p>
                  <p>
                    {domain.snapshot.enabled === undefined
                      ? i18n.t("Sending status not checked")
                      : domain.snapshot.enabled
                        ? i18n.t("Enabled")
                        : i18n.t("Disabled")}{" "}
                    ·{" "}
                    {domain.snapshot.default === undefined
                      ? i18n.t("Domain mode not checked")
                      : domain.snapshot.default
                        ? i18n.t("Default domain")
                        : i18n.t("Dedicated domain")}
                  </p>
                  {domain.snapshot.domain_name &&
                    domain.snapshot.verification_domain && (
                      <>
                        <p>{i18n.t("Add both CNAME records at your DNS host, then verify. Your DNS host may append the parent domain automatically.")}</p>
                        <dl>
                          {[
                            domain.snapshot.domain_name,
                            domain.snapshot.verification_domain,
                          ].map((name) => (
                            <div key={name}>
                              <dt>{i18n.t("CNAME name")}</dt>
                              <dd style={{ overflowWrap: "anywhere" }}>
                                <code>{name}</code>
                              </dd>
                              <dt>{i18n.t("CNAME target")}</dt>
                              <dd>
                                <code>smtptrack.com</code>
                              </dd>
                            </div>
                          ))}
                        </dl>
                      </>
                    )}
                  {editable && (
                    <>
                      <TrackingForm
                        customer={customerID} connection={connectionID}
                        action="verify"
                        label={i18n.t("Verify tracking domain")}
                      >
                        <input
                          type="hidden"
                          name="domainID"
                          value={domain.id}
                        />
                      </TrackingForm>
                      {(
                        [
                          [
                            "enabled",
                            "Domain enabled",
                            domain.snapshot.enabled,
                          ],
                          [
                            "dedicated",
                            msgid("Dedicated to this sender domain"),
                            domain.snapshot.default === undefined
                              ? undefined
                              : !domain.snapshot.default,
                          ],
                        ] as const
                      ).map(([action, label, enabled]) => (
                        <TrackingForm
                          key={action}
                          customer={customerID} connection={connectionID}
                          action={action}
                          label={i18n.t("Update {setting}",{setting:i18n.t(label)})}
                        >
                          <input
                            type="hidden"
                            name="domainID"
                            value={domain.id}
                          />
                          <label className="field">
                            {i18n.t(label)}
                            <select
                              name="value"
                              required
                              defaultValue={
                                enabled === undefined
                                  ? ""
                                  : enabled
                                    ? "yes"
                                    : "no"
                              }
                            >
                              <option value="" disabled>{i18n.t("Select a state")}</option>
                              <option value="yes">{i18n.t("Yes")}</option>
                              <option value="no">{i18n.t("No")}</option>
                            </select>
                          </label>
                        </TrackingForm>
                      ))}
                      <TrackingForm
                        customer={customerID} connection={connectionID}
                        action="remove"
                        label={i18n.t("Remove tracking domain")}
                        confirm={i18n.t("Remove this tracking domain from the sending account. Existing tracked links may stop working.")}
                      >
                        <input
                          type="hidden"
                          name="domainID"
                          value={domain.id}
                        />
                      </TrackingForm>
                    </>
                  )}
                </>
              )}
            </article>
          ))}
        </div>
        {!data.domains.length && <p>{i18n.t("No custom tracking domains yet.")}</p>}
      </section>
    </div>
  );
}
