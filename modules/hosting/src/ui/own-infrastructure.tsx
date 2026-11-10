import * as React from "react";
import type { OwnInfrastructureView } from "../ui-types";
import Link from "next/link";
import { randomUUID } from "node:crypto";
import { getRequestI18n } from "@spitzli/i18n/next";
import { HostingForm } from "./form";
import { HostingTable } from "./table";
import type { HostingUIServices } from "./services";
export function createOwnInfrastructureView(
  services: Pick<HostingUIServices, "hostingPageCall">,
) {
  const { hostingPageCall } = services;
  async function OwnInfrastructure({
    customerID,
    path,
  }: {
    customerID: string;
    path: string;
  }) {
    const { t } = await getRequestI18n();
    const data = await hostingPageCall<OwnInfrastructureView>(
        { action: "byok.list", customerID },
        path,
      ),
      p = data.policy;
    if (
      !data.operator &&
      !p.kubernetes &&
      !p.vercel &&
      !p.turbosmtp &&
      !data.clusters.length &&
      !data.vercel
    )
      return null;
    return (
      <section className="panel hosting-own">
        <div className="page-heading">
          <div>
            <h2>{t("Your infrastructure")}</h2>
            <p>
              {t(
                "Connect your own Kubernetes cluster, Vercel team or turboSMTP account. Your provider bills you directly.",
              )}
            </p>
          </div>
        </div>
        {data.operator && (
          <details>
            <summary>{t("Selfservice permissions")}</summary>
            <HostingForm
              preserveRevision
              command={{ action: "byok.policy.set", customerID, ...p }}
              label={t("Save permissions")}
            >
              <div className="hosting-permissions">
                {(
                  [
                    "kubernetes",
                    "vercel",
                    "turbosmtp",
                    "manageExisting",
                  ] as const
                ).map((key, i) => (
                  <label className="check" key={key}>
                    <input
                      type="checkbox"
                      name={key}
                      value="yes"
                      defaultChecked={p[key]}
                    />
                    <span>
                      {
                        [
                          t("Allow own Kubernetes clusters"),
                          t("Allow own Vercel team"),
                          t("Allow own turboSMTP account"),
                          t("Manage existing applications"),
                        ][i]
                      }
                    </span>
                  </label>
                ))}
              </div>
              <div className="plan-fields">
                <label className="field">
                  {t("Maximum clusters")}
                  <input
                    type="number"
                    name="maxClusters"
                    min="0"
                    max="100"
                    defaultValue={p.maxClusters}
                    required
                  />
                </label>
                <label className="field">
                  {t("Maximum turboSMTP sender domains")}
                  <input
                    type="number"
                    name="maxMailDomains"
                    min="0"
                    max="1000"
                    defaultValue={p.maxMailDomains}
                    required
                  />
                  <small>
                    {t(
                      "Limits new registrations through Webdock. Existing domains and provider sending limits stay unchanged.",
                    )}
                  </small>
                </label>
                <label className="field wide">
                  {t("Allowed namespaces")}
                  <input
                    name="namespaces"
                    defaultValue={p.namespaces.join(", ")}
                    placeholder="apps, production"
                  />
                  <small>
                    {t(
                      "Separate names with commas. System namespaces stay protected.",
                    )}
                  </small>
                </label>
              </div>
            </HostingForm>
          </details>
        )}
        {!p.kubernetes && !p.vercel && !p.turbosmtp && (
          <p className="empty-state">
            {t(
              "Your plan does not yet include own infrastructure. Your administrator can enable it here.",
            )}
          </p>
        )}
        <div className="hosting-provider-grid">
          {p.turbosmtp && (
            <section>
              <h3>turboSMTP</h3>
              <p>
                {t(
                  "Connect your own sending account and manage its sender domains.",
                )}
              </p>
              <Link
                className="button secondary"
                href={`/tenants/${customerID}/mail/settings#own-mail`}
              >
                {t("Manage own email")}
              </Link>
            </section>
          )}
          {p.kubernetes && (
            <section>
              <h3>Kubernetes / k3s</h3>
              <p>
                {t("Connected clusters")}: {data.clusters.length} /{" "}
                {p.maxClusters}
              </p>
              <HostingTable
                rows={data.clusters}
                rowKey={(r) => r.id}
                empty={t("No cluster connected yet.")}
                columns={[
                  {
                    label: t("Name"),
                    render: (r) => (
                      <Link href={`/hosting/clusters/${r.id}`}>{r.name}</Link>
                    ),
                  },
                  {
                    label: t("Status"),
                    render: (r) => (
                      <span className={`hosting-state ${r.state}`}>
                        {t(r.state)}
                      </span>
                    ),
                  },
                ]}
              />
              {data.canWrite && data.clusters.length < p.maxClusters && (
                <details>
                  <summary>{t("Connect own cluster")}</summary>
                  <HostingForm
                    command={{
                      action: "byok.register",
                      customerID,
                      name: "",
                      provider: "kubernetes",
                      region: "EU",
                      locationEvidence: "",
                      idempotencyKey: randomUUID(),
                    }}
                    label={t("Add cluster")}
                  >
                    <label className="field">
                      {t("Cluster name")}
                      <input name="name" required maxLength={160} />
                    </label>
                    <label className="field">
                      {t("Cluster type")}
                      <select name="provider">
                        <option value="kubernetes">Kubernetes</option>
                        <option value="k3s">k3s</option>
                      </select>
                    </label>
                    <label className="field">
                      {t("EU location")}
                      <input
                        name="region"
                        required
                        placeholder={t("For example, Frankfurt")}
                      />
                    </label>
                    <label className="field">
                      {t("Location evidence")}
                      <input
                        name="locationEvidence"
                        required
                        maxLength={2000}
                      />
                      <small>
                        {t(
                          "Enter your provider location evidence. A declaration is not provider verification.",
                        )}
                      </small>
                    </label>
                  </HostingForm>
                </details>
              )}
            </section>
          )}
          {p.vercel && (
            <section>
              <h3>Vercel</h3>
              <p>
                {data.vercel
                  ? t("Connected")
                  : t("Connect your own Vercel team to choose its projects.")}
              </p>
              {data.vercel && (
                <Link
                  className="button secondary"
                  href={`/hosting/vercel/${customerID}`}
                >
                  {t("Manage Vercel projects")}
                </Link>
              )}
              {data.canWrite && (
                <form action="/api/hosting/vercel/connect" method="post">
                  <input type="hidden" name="customerID" value={customerID} />
                  <button className="button">
                    {data.vercel ? t("Reconnect Vercel") : t("Connect Vercel")}
                  </button>
                </form>
              )}
              <small>
                {t(
                  "Provider billing and Webdock allowances are separate. Budgets shown here are not a Vercel spending cap.",
                )}
              </small>
            </section>
          )}
        </div>
      </section>
    );
  }
  return { OwnInfrastructure };
}
