import * as React from "react";
import { filterRows } from "@spitzli/search";
import { LiveSearchForm } from "@spitzli/search/form";
import Link from "next/link";
import { getRequestI18n } from "@spitzli/i18n/next";
import { HostingForm } from "./form";
import { HostingTable } from "./table";
import type { HostingUIServices } from "./services";
export function createOwnMailView(
  services: Pick<HostingUIServices, "hostingPageCall">,
) {
  const { hostingPageCall } = services;
  type OwnMailView = {
    connected: boolean;
    enabled: boolean;
    label: string;
    keySuffix: string | null;
    revision: number;
    maxDomains: number;
    operator: boolean;
    canWrite: boolean;
    checkedAt: string | null;
    state: string;
    pendingDomain: string | null;
    domains: {
      id: string;
      domain: string;
      spf_verified: boolean;
      dkim_verified: boolean;
      dmarc_verified: boolean;
    }[];
    smtpHost: string;
    sendAPI: string;
  };
  async function OwnMail({
    customerID,
    query = {},
  }: {
    customerID: string;
    query?: { mailPage?: string; mailSearch?: string };
  }) {
    const { t, date, number } = await getRequestI18n();
    const data = await hostingPageCall<OwnMailView>(
      { action: "byok.mail.get", customerID },
      `/tenants/${customerID}/mail/settings`,
    );
    if (!data.operator && !data.enabled && !data.connected) return null;
    const page = Math.max(
      1,
      Math.min(20, Math.floor(Number(query.mailPage)) || 1),
    );
    const search = (query.mailSearch ?? "").slice(0, 160);
    const domains = filterRows(data.domains, search, (d) => d.domain);
    return (
      <details
        id="own-mail"
        className="panel"
        open={data.enabled || data.connected}
      >
        <summary>{t("Own sending connection")}</summary>
        <div className="page-heading">
          <div>
            <h2>{t("Your own turboSMTP account")}</h2>
            <p>
              {t(
                "Use your own provider account. turboSMTP bills you directly; Webdock manages the connection and sender domains.",
              )}
            </p>
          </div>
          {data.operator && (
            <Link
              className="button secondary"
              href={`/customers/${customerID}/hosting`}
            >
              {t("Manage selfservice permissions")}
            </Link>
          )}
        </div>
        {!data.enabled && (
          <p role="status">
            {t("Own turboSMTP is not enabled for this customer.")}
          </p>
        )}
        {data.connected ? (
          <>
            <dl className="allowance-list">
              <div>
                <dt>{t("Connection")}</dt>
                <dd>{data.label}</dd>
              </div>
              <div>
                <dt>{t("Key ending")}</dt>
                <dd>•••• {data.keySuffix}</dd>
              </div>
              <div>
                <dt>{t("Sender domains")}</dt>
                <dd>
                  {number(data.domains.length)} / {number(data.maxDomains)}
                </dd>
              </div>
              <div>
                <dt>{t("Last checked")}</dt>
                <dd>
                  {data.checkedAt
                    ? date(data.checkedAt, {
                        dateStyle: "medium",
                        timeStyle: "short",
                      })
                    : t("Unavailable")}
                </dd>
              </div>
            </dl>
            {data.pendingDomain && (
              <p role="alert">
                {t(
                  "A sender-domain registration needs review. Refresh to check whether the provider completed it.",
                )}{" "}
                {data.pendingDomain}
              </p>
            )}
            {data.enabled && data.canWrite && (
              <HostingForm
                command={{
                  action: "byok.mail.refresh",
                  customerID,
                  revision: data.revision,
                }}
                label={t("Check connection and domains")}
              />
            )}
            <LiveSearchForm
              className="hosting-filters"
              path={`/tenants/${customerID}/mail/settings`}
              pageKey="mailPage"
            >
              <label>
                {t("Search domains")}
                <input
                  name="mailSearch"
                  type="search"
                  data-search-default=""
                  maxLength={160}
                  defaultValue={search}
                />
              </label>
              <noscript>
                <button className="button secondary">{t("Search")}</button>
              </noscript>
            </LiveSearchForm>
            <HostingTable
              rows={domains.slice((page - 1) * 50, page * 50)}
              rowKey={(r) => r.id}
              empty={t("No sender domains found in this account.")}
              columns={[
                { label: t("Domain"), render: (r) => r.domain },
                ...(
                  ["spf_verified", "dkim_verified", "dmarc_verified"] as const
                ).map((key, i) => ({
                  label: ["SPF", "DKIM", "DMARC"][i],
                  render: (r: OwnMailView["domains"][number]) =>
                    r[key] ? t("Verified") : t("Pending"),
                })),
              ]}
            />
            <nav className="hosting-pagination">
              {page > 1 && (
                <Link
                  href={`?${new URLSearchParams({ mailSearch: search, mailPage: String(page - 1) })}#own-mail`}
                >
                  {t("Previous")}
                </Link>
              )}
              <span>
                {number(domains.length)} {t("Sender domains")}
              </span>
              {page * 50 < domains.length && (
                <Link
                  href={`?${new URLSearchParams({ mailSearch: search, mailPage: String(page + 1) })}#own-mail`}
                >
                  {t("Next")}
                </Link>
              )}
            </nav>
            {data.domains.length >= data.maxDomains && (
              <p>
                {t(
                  "The domain allowance for this connection has been reached.",
                )}
              </p>
            )}
            {data.enabled &&
              data.canWrite &&
              !data.pendingDomain &&
              data.domains.length < data.maxDomains && (
                <details>
                  <summary>{t("Add sender domain")}</summary>
                  <p>
                    {t(
                      "Registration starts provider verification. Configure the DNS records shown in your turboSMTP dashboard, then refresh here.",
                    )}
                  </p>
                  <HostingForm
                    command={{
                      action: "byok.mail.domain",
                      customerID,
                      revision: data.revision,
                      domain: "",
                    }}
                    label={t("Register sender domain")}
                  >
                    <label className="field">
                      {t("Domain")}
                      <input
                        name="domain"
                        required
                        maxLength={253}
                        placeholder="example.com"
                        autoCapitalize="none"
                        autoCorrect="off"
                        spellCheck={false}
                      />
                    </label>
                  </HostingForm>
                </details>
              )}
            <details>
              <summary>{t("EU sending settings")}</summary>
              <dl>
                <dt>SMTP</dt>
                <dd>
                  <code>{data.smtpHost}</code>
                </dd>
                <dt>{t("Sending API")}</dt>
                <dd>
                  <code>{data.sendAPI}</code>
                </dd>
              </dl>
              <p>
                {t(
                  "Use your provider-issued sending credentials in your application. This connection does not send a test email or change existing application settings.",
                )}
              </p>
              <p>
                {t(
                  "Manage sending keys and account billing in turboSMTP. This management key is never displayed again by Webdock.",
                )}
              </p>
            </details>
            {data.canWrite && !data.pendingDomain && (
              <details>
                <summary>{t("Disconnect account")}</summary>
                <p>
                  {t(
                    "Disconnecting removes the stored Webdock credential. Your turboSMTP account, domains and applications keep running. Revoke the key at turboSMTP if it should lose provider access too.",
                  )}
                </p>
                <HostingForm
                  command={{
                    action: "byok.mail.disconnect",
                    customerID,
                    revision: data.revision,
                  }}
                  label={t("Disconnect turboSMTP")}
                />
              </details>
            )}
            {data.operator && data.pendingDomain && (
              <details>
                <summary>{t("Review uncertain registration")}</summary>
                <p>
                  {t(
                    "Wait at least five minutes and inspect the provider state. Confirm the exact pending domain to allow a new operation.",
                  )}
                </p>
                <HostingForm
                  command={{
                    action: "byok.mail.review",
                    customerID,
                    revision: data.revision,
                    confirmDomain: "",
                  }}
                  label={t("Confirm reviewed state")}
                >
                  <label className="field">
                    {t("Domain")}
                    <input name="confirmDomain" required autoComplete="off" />
                  </label>
                </HostingForm>
              </details>
            )}
          </>
        ) : (
          <p className="empty-state">
            {t("No own turboSMTP account connected yet.")}
          </p>
        )}
        {data.enabled && data.canWrite && !data.pendingDomain && (
          <details open={!data.connected}>
            <summary>
              {data.connected
                ? t("Replace management key")
                : t("Connect turboSMTP")}
            </summary>
            <p>
              {t(
                "Create a dedicated Consumer Key and Consumer Secret with API management permission in turboSMTP. Use a separate account for this customer; all sender domains in that account will be visible here.",
              )}
            </p>
            <HostingForm
              command={{
                action: "byok.mail.connect",
                customerID,
                revision: data.revision,
                label: "",
                consumerKey: "",
                consumerSecret: "",
                confirmAccountAccess: true,
              }}
              label={t("Verify and save connection")}
            >
              <label className="field">
                {t("Connection name")}
                <input
                  name="label"
                  defaultValue={data.label}
                  required
                  maxLength={160}
                  autoComplete="off"
                />
              </label>
              <label className="field">
                Consumer Key
                <input
                  name="consumerKey"
                  type="password"
                  required
                  maxLength={4096}
                  autoComplete="off"
                />
              </label>
              <label className="field">
                Consumer Secret
                <input
                  name="consumerSecret"
                  type="password"
                  required
                  maxLength={4096}
                  autoComplete="new-password"
                />
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  name="confirmAccountAccess"
                  value="yes"
                  required
                />
                {t(
                  "I control this provider account and authorize this customer to manage its sender domains.",
                )}
              </label>
            </HostingForm>
          </details>
        )}
        <p className="help">
          {t(
            "Sending uses EU endpoints. The separate provider management API is an approved exception without an EU-only guarantee. The Webdock domain allowance is not an email sending or spending limit.",
          )}
        </p>
      </details>
    );
  }
  return { OwnMail };
}
