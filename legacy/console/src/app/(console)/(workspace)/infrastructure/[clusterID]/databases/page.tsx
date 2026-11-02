import { WorkspaceContext } from "@/components/workspace-context";
import { AreaNavigation } from "@/components/area-navigation";
import { randomUUID } from "node:crypto";
import { getRequestI18n } from "@webdock/i18n/next";
import {
  formatResource,
  type HostingClusterView,
} from "@webdock/hosting-contracts";
import { hostingPageCall } from "@/lib/hosting-client";
import {
  DatabasePlanForm,
  type DatabaseCatalogEntry,
} from "@/components/hosting/databases";
import { HostingTable } from "@/components/hosting/table";
import { HostingForm } from "@/components/hosting/form";
import { HostingRefresh } from "@/components/hosting/refresh";
type Backend = {
  id: string;
  kind: string;
  node: string;
  state: string;
  revision: number;
  observedAt: string | null;
  demand: { cpuMillicores: number; memoryBytes: number; volumeBytes: number };
  maxDatabases: number;
};
export default async function OperatorDatabases({
  params,
  searchParams,
}: {
  params: Promise<{ clusterID: string }>;
  searchParams: Promise<{ view?: string }>;
}) {
  const { clusterID } = await params,
    path = `/infrastructure/${clusterID}/databases`;
  const { t, locale } = await getRequestI18n();
  const [cluster, backends, plans] = await Promise.all([
    hostingPageCall<HostingClusterView>(
      { action: "clusters.get", clusterID },
      path,
    ),
    hostingPageCall<Backend[]>(
      { action: "databases.installations.list", clusterID },
      path,
    ),
    hostingPageCall<DatabaseCatalogEntry[]>(
      { action: "databases.plans.list" },
      path,
    ),
  ]);
  const view = (await searchParams).view === "catalog" ? "catalog" : "overview";
  const pools = cluster.observation?.capabilities.databasePools ?? [];
  return (
    <>
      <WorkspaceContext
        items={[
          { label: t("Operations"), href: "/operations" },
          { label: t("Infrastructure"), href: "/infrastructure" },
          { label: cluster.name, href: `/infrastructure/${clusterID}` },
          { label: t("Managed PostgreSQL") },
        ]}
      />
      <div className="page-heading">
        <h1>{t("Managed PostgreSQL")}</h1>
      </div>
      <AreaNavigation
        links={[
          {
            href: path + "?view=overview",
            label: t("Database installations"),
            exact: true,
          },
          {
            href: path + "?view=catalog",
            label: t("Database catalog"),
            exact: true,
          },
        ]}
      />
      <HostingRefresh
        interval={75000}
        active={backends.some((b) => b.state === "provisioning")}
      />
      {view === "overview" && (
        <>
          <section className="panel">
            <h2>{t("Database installations")}</h2>
            <p>
              {t(
                "Physical resources are reserved once per installation. Paused and retained resources keep their reservation.",
              )}
            </p>
            <HostingTable
              rows={backends}
              rowKey={(b) => b.id}
              empty={t("No PostgreSQL installations.")}
              columns={[
                { label: t("ID"), render: (b) => b.id },
                {
                  label: t("Database kind"),
                  render: (b) =>
                    b.kind === "shared-pool"
                      ? t("Shared pool")
                      : t("Dedicated instance"),
                },
                { label: t("Node"), render: (b) => b.node },
                { label: t("Status"), render: (b) => t(b.state) },
                {
                  label: t("Memory"),
                  render: (b) =>
                    formatResource("memoryBytes", b.demand.memoryBytes, locale),
                },
                {
                  label: t("Persistent storage"),
                  render: (b) =>
                    formatResource("volumeBytes", b.demand.volumeBytes, locale),
                },
              ]}
            />
          </section>
          <details className="workspace-disclosure">
            <summary>{t("Create shared pool")}</summary>
            <p>
              {t(
                "The shared profile reserves PostgreSQL, pooler and job resources. Customer orders use separate logical slots.",
              )}
            </p>
            {cluster.workloadReady && pools.some((p) => p.ready) ? (
              pools
                .filter((p) => p.ready)
                .map((pool) => (
                  <div key={pool.poolID}>
                    <p>
                      {pool.node} ·{" "}
                      {formatResource(
                        "volumeBytes",
                        pool.capacityBytes,
                        locale,
                      )}
                    </p>
                    <HostingForm
                      label={t("Reserve observed database pool")}
                      command={{
                        action: "databases.pools.register",
                        clusterID,
                        node: pool.node,
                        poolID: pool.poolID,
                        revision: cluster.revision,
                        idempotencyKey: randomUUID(),
                      }}
                    />
                    <HostingForm
                      label={t("Provision shared pool")}
                      command={{
                        action: "databases.installations.create",
                        clusterID,
                        node: pool.node,
                        poolID: pool.poolID,
                        profile: "shared",
                        idempotencyKey: randomUUID(),
                      }}
                    />
                  </div>
                ))
            ) : (
              <p>{t("No qualified PostgreSQL storage agent is available.")}</p>
            )}
          </details>
        </>
      )}
      {view === "catalog" && (
        <section className="panel">
          <h2>{t("Database catalog")}</h2>
          <p>
            {t(
              "Plan terms and prices are immutable. Create a new revision to change them; existing orders keep their accepted terms.",
            )}
          </p>
          {plans.map((p) => (
            <details key={p.id} className="workspace-disclosure">
              <summary>
                {p.plan.name} ·{" "}
                {new Intl.NumberFormat(locale, {
                  style: "currency",
                  currency: "EUR",
                }).format(p.plan.monthlyPriceCents / 100)}{" "}
                · {t(p.availability)}
              </summary>
              <p>{p.plan.terms}</p>
              <HostingForm
                label={t("Set plan availability")}
                command={{
                  action: "databases.plans.setAvailability",
                  planRevisionID: p.id,
                  revision: p.revision,
                  availability: p.availability,
                  idempotencyKey: randomUUID(),
                }}
              >
                <label className="field">
                  {t("Availability")}
                  <select name="availability" defaultValue={p.availability}>
                    <option value="disabled">{t("Disabled")}</option>
                    <option value="pilot">{t("Operator pilot")}</option>
                    <option value="public">{t("Customer self-service")}</option>
                  </select>
                </label>
              </HostingForm>
            </details>
          ))}
          <details className="workspace-disclosure">
            <summary>{t("Create disabled plan")}</summary>
            <DatabasePlanForm requestKey={randomUUID()} />
          </details>
        </section>
      )}
      <details className="workspace-disclosure">
        <summary>{t("Backup TODO")}</summary>
        <p>
          {t(
            "External backups, WAL archiving, PITR and restore verification are deferred. No backup service is active.",
          )}
        </p>
        <p>
          {t("Public ordering requires a separate product release decision.")}
        </p>
      </details>
    </>
  );
}
