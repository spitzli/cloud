import * as React from "react";
import { randomUUID } from "node:crypto";
import Link from "next/link";
import { getRequestI18n } from "@spitzli/i18n/next";
import {
  formatResource,
  hostingDimensions,
  hostingLabels,
  type HostingAllowances,
  type HostingClusterView,
  type HostingProjectView,
  type HostingPage,
  type HostingUsageView,
} from "@spitzli/hosting-contracts";
import { HostingTable } from "./table";
import { HostingForm } from "./form";
import { HostingAllowanceFields } from "./allowances";
import { Pagination, ListControls } from "@spitzli/console-kit/list-controls";
import type { HostingUIServices } from "./services";
export function createCustomerView(
  services: Pick<
    HostingUIServices,
    "getAvailableProjects" | "OwnInfrastructure" | "hostingPageCall"
  >,
) {
  const { getAvailableProjects, OwnInfrastructure, hostingPageCall } = services;
  async function CustomerHosting({
    customerID,
    path,
    page = 1,
    search = "",
    sort = "name",
  }: {
    customerID: string;
    path: string;
    page?: number;
    search?: string;
    sort?: "name" | "-name";
  }) {
    const { t, locale } = await getRequestI18n();
    const [usage, projects] = await Promise.all([
      hostingPageCall<HostingUsageView>(
        { action: "usage.get", customerID },
        path,
      ),
      hostingPageCall<HostingPage<HostingProjectView>>(
        {
          action: "projects.list",
          customerID,
          page,
          limit: 20,
          search,
          sort,
        },
        path,
      ),
    ]);
    const operator = projects.operator === true;
    let availableProjects: { id: string; name: string }[] = [];
    let availableClusters: HostingClusterView[] = [];
    if (operator) {
      availableProjects = await getAvailableProjects(customerID);
      availableClusters = (
        await hostingPageCall<HostingPage<HostingClusterView>>(
          { action: "clusters.list", page: 1, limit: 50, search: "" },
          path,
        )
      ).docs.filter(
        (c) =>
          c.workloadReady &&
          (!c.dedicatedCustomerID || c.dedicatedCustomerID === customerID),
      );
    }
    const scopes = [
      "customer",
      "provider:k3s",
      "provider:vercel",
      ...projects.docs.map((p) => "project:" + p.projectID),
    ];
    return (
      <>
        <div className="page-heading">
          <div>
            <p className="eyebrow">{t("Hosting")}</p>
            <h1>{t("Applications and hosting")}</h1>
            <p>
              {t(
                "Allocated resources, reservations and measured usage are tracked separately.",
              )}
            </p>
          </div>
          <Link
            href={`/tenants/${customerID}/usage`}
            className="button secondary"
          >
            {t("Plan & usage")}
          </Link>
        </div>
        <section className="panel">
          <h2>{t("Hosting projects")}</h2>
          <ListControls
            path={path}
            q={search}
            sort={sort}
            placeholder={t("Search")}
            sortOptions={[
              { value: "name", label: "Name A–Z" },
              { value: "-name", label: "Name Z–A" },
            ]}
          />
          <HostingTable
            rows={projects.docs}
            rowKey={(r) => r.projectID}
            empty={t("No hosting projects assigned.")}
            columns={[
              {
                label: t("Project"),
                render: (r) => (
                  <Link href={`/hosting/projects/${r.projectID}`}>
                    {r.name}
                  </Link>
                ),
              },
              { label: t("Provider"), render: (r) => r.provider },
              {
                label: t("Mode"),
                render: (r) =>
                  r.mode === "managed" ? t("Managed") : t("Selfservice"),
              },
              { label: t("Cluster"), render: (r) => r.clusterName ?? "—" },
            ]}
          />
          <Pagination path={path} query={{ q: search, sort }} {...projects} />
          <p>{t("Open a hosting project to manage its applications.")}</p>
        </section>
        <OwnInfrastructure customerID={customerID} path={path} />
        {projects.totalDocs > 0 && (
          <section className="panel">
            <h2>{t("Webdock hosting allowances")}</h2>
            {usage.overallocated && (
              <p role="status">
                {t(
                  "Existing allocations exceed the current limit. New allocations are blocked; existing workloads remain intact.",
                )}
              </p>
            )}
            <HostingTable
              rows={[...hostingDimensions]}
              rowKey={(r) => r}
              empty=""
              columns={[
                { label: t("Resource"), render: (k) => t(hostingLabels[k]) },
                {
                  label: t("Limit"),
                  render: (k) =>
                    usage.effective[k] === null
                      ? t("Unlimited")
                      : formatResource(k, usage.effective[k], locale),
                },
                {
                  label: t("Reserved"),
                  render: (k) => formatResource(k, usage.reserved[k], locale),
                },
                {
                  label: t("Allocated"),
                  render: (k) => formatResource(k, usage.allocated[k], locale),
                },
              ]}
            />
            <p className="muted">
              {t(
                "Measured usage is not available yet. These values describe limits and allocations.",
              )}
            </p>
            <p>
              {t(
                "These allowances apply to Webdock-hosted applications. Your own infrastructure is managed separately.",
              )}
            </p>
          </section>
        )}
        {operator && (
          <>
            <details className="workspace-disclosure">
              <summary>{t("Assign project hosting")}</summary>
              <HostingForm
                command={{
                  action: "projects.create",
                  projectID: "1",
                  provider: "k3s",
                  mode: "managed",
                  ownImages: false,
                  confirmSharedImages: false,
                  idempotencyKey: randomUUID(),
                }}
              >
                <label className="field">
                  {t("Project")}
                  <select name="projectID" required>
                    <option value="">{t("Choose a project")}</option>
                    {availableProjects.map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.name}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="field">
                  {t("Provider")}
                  <select name="provider">
                    <option value="k3s">k3s</option>
                    <option value="vercel">Vercel</option>
                  </select>
                </label>
                <label className="field">
                  {t("Cluster")}
                  <select name="clusterID">
                    <option value="">
                      {t("Choose a cluster for Kubernetes")}
                    </option>
                    {availableClusters.map((c) => (
                      <option key={c.id} value={c.id}>
                        {c.name}
                      </option>
                    ))}
                  </select>
                </label>
                <ModeFields t={t} />
              </HostingForm>
            </details>
            <details className="workspace-disclosure">
              <summary>{t("Limit overrides")}</summary>
              <p>
                {t(
                  "Overrides can restrict the package allowance; they cannot increase it. Unlimited here means no additional cap.",
                )}
              </p>
              {scopes.map((scope) => {
                const existing = usage.limits.find((l) => l.scope === scope);
                const values = Object.fromEntries(
                  hostingDimensions.map((k) => [
                    k,
                    existing?.values[k] ?? null,
                  ]),
                ) as HostingAllowances;
                return (
                  <details key={scope}>
                    <summary>
                      {scope === "customer"
                        ? t("Customer")
                        : scope === "provider:k3s"
                          ? "Kubernetes / k3s"
                          : scope === "provider:vercel"
                            ? "Vercel"
                            : (projects.docs.find(
                                (p) => scope === "project:" + p.projectID,
                              )?.name ?? t("Project"))}
                    </summary>
                    <HostingForm
                      command={{
                        action: "limits.set",
                        customerID,
                        values,
                        revision: existing?.revision ?? 0,
                        subscriptionRevision: usage.subscriptionRevision,
                      }}
                    >
                      <input type="hidden" name="scope" value={scope} />
                      <HostingAllowanceFields values={values} />
                    </HostingForm>
                  </details>
                );
              })}
            </details>
            {projects.docs.map((p) => (
              <section className="panel" key={p.projectID}>
                <h2>{p.name}</h2>
                <HostingForm
                  command={{
                    action: "projects.update",
                    projectID: p.projectID,
                    mode: p.mode,
                    ownImages: p.ownImages,
                    confirmSharedImages: false,
                    revision: p.revision,
                  }}
                >
                  <ModeFields t={t} mode={p.mode} ownImages={p.ownImages} />
                </HostingForm>
              </section>
            ))}
          </>
        )}
      </>
    );
  }
  function ModeFields({
    t,
    mode = "managed",
    ownImages = false,
  }: {
    t: (key: string) => string;
    mode?: string;
    ownImages?: boolean;
  }) {
    return (
      <>
        <label className="field">
          {t("Mode")}
          <select name="mode" defaultValue={mode}>
            <option value="managed">{t("Managed")}</option>
            <option value="selfservice">{t("Selfservice")}</option>
          </select>
        </label>
        <input type="hidden" name="ownImagesPresent" value="yes" />
        <label className="check">
          <input
            type="checkbox"
            name="ownImages"
            value="yes"
            defaultChecked={ownImages}
          />
          {t("Allow own container images")}
        </label>
        <label className="check">
          <input type="checkbox" name="confirmSharedImages" value="yes" />
          {t(
            "Also allow this project's images on a shared cluster (applications share the host kernel)",
          )}
        </label>
      </>
    );
  }
  return { CustomerHosting };
}
