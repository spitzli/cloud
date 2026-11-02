import type { OwnClusterView, OwnInventoryView } from "@/lib/hosting-views";
import {LiveSearchForm} from "@webdock/search/form";
import { uiLabel } from "@/lib/ui-labels";
import Link from "next/link";
import { randomUUID } from "node:crypto";
import { getRequestI18n } from "@webdock/i18n/next";
import {
  formatResource,
  type InventoryResource,
  resourceKindSchema,
} from "@webdock/hosting-contracts";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingForm } from "@/components/hosting/form";
import { HostingTable } from "@/components/hosting/table";
import { HostingRefresh } from "@/components/hosting/refresh";
export default async function OwnCluster({
  params,
  searchParams,
}: {
  params: Promise<{ clusterID: string }>;
  searchParams: Promise<{
    page?: string;
    search?: string;
    namespace?: string;
    kind?: string;
    view?: string;
  }>;
}) {
  const { clusterID } = await params,
    q = await searchParams,
    path = `/hosting/clusters/${clusterID}`,
    { t, locale } = await getRequestI18n();
  const page = Math.max(1, Math.min(100, Number(q.page) || 1));
  const [c, inventory] = await Promise.all([
    hostingPageCall<OwnClusterView>({ action: "byok.cluster", clusterID }, path),
    hostingPageCall<OwnInventoryView>(
      {
        action: "byok.inventory",
        view: q.view === "all" ? "all" : "applications",
        clusterID,
        page,
        search: q.search ?? "",
        namespace: q.namespace || undefined,
        kind: resourceKindSchema.safeParse(q.kind).data,
      },
      path,
    ),
  ]);
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>{c.name}</h1>
          <p>
            <span className={`hosting-state ${c.state}`}>{t(c.state)}</span> ·{" "}
            {t("Customer-owned cluster")}
          </p>
        </div>
        <Link href={`/tenants/${c.dedicatedCustomerID}/hosting`}>
          {t("Hosting")}
        </Link>
      </div>
      <HostingRefresh active={true} />
      {c.state === "unconnected" && (
        <section className="panel">
          <h2>{t("Connect your cluster")}</h2>
          <p>
            {t(
              "Install the agent on a host with access to your cluster. Your Kubernetes credentials stay on that host.",
            )}
          </p>
          {c.canWrite && (
            <HostingForm
              command={{ action: "byok.enrollment", clusterID }}
              label={t("Set up connection")}
            />
          )}
        </section>
      )}
      <section className="panel">
        <div className="page-heading">
          <div>
            <h2>
              {q.view === "all" ? t("Cluster resources") : t("Applications")}
            </h2>
            <p>
              {inventory.observedAt
                ? new Date(inventory.observedAt).toLocaleString("de-DE")
                : t("Waiting for first scan")}
            </p>
          </div>
          {c.canWrite && (
            <HostingForm
              command={{
                action: "byok.scan",
                clusterID,
                idempotencyKey: randomUUID(),
              }}
              label={t("Refresh inventory")}
            />
          )}
        </div>
        {inventory.stale && (
          <p role="status">
            {t(
              "This inventory is out of date. Refresh it before making changes.",
            )}
          </p>
        )}
        {!inventory.complete && inventory.observedAt && (
          <p role="status">
            {t(
              "Some resources could not be scanned. Check the agent permissions.",
            )}
          </p>
        )}
        {inventory.unavailable.includes("metrics") && (
          <p>
            {t(
              "Live CPU and memory usage is unavailable. Resource requests are still shown.",
            )}
          </p>
        )}
        <nav className="hosting-project-links">
          <Link href={path}>{t("Applications")}</Link>
          <Link href={`${path}?view=all`}>{t("All resources")}</Link>
        </nav>
        <details open={Boolean(q.search||q.namespace||q.kind)}><summary>{t("Search and filters")}</summary>
        <LiveSearchForm className="hosting-filters" path={path}>
          <input
            type="hidden"
            name="view"
            value={q.view === "all" ? "all" : "applications"}
          />
          <label>
            {t("Search")}
            <input name="search" type="search" maxLength={160} data-search-default="" defaultValue={q.search} />
          </label>
          <label>
            {t("Namespace")}
            <select name="namespace" defaultValue={q.namespace ?? ""} data-search-default="">
              <option value="">{t("All allowed namespaces")}</option>
              {c.policy.namespaces.map((n: string) => (
                <option key={n}>{n}</option>
              ))}
            </select>
          </label>
          <label>
            {t("Resource type")}
            <select name="kind" defaultValue={q.kind ?? ""} data-search-default="">
              <option value="">{t("All resource types")}</option>
              {resourceKindSchema.options.map((k) => (
                <option key={k}>{k}</option>
              ))}
            </select>
          </label>
          <noscript><button className="button secondary">{t("Filter")}</button></noscript>
        </LiveSearchForm></details>
        <HostingTable
          rows={inventory.resources as InventoryResource[]}
          rowKey={(r) => r.uid}
          empty={t("No resources match this view.")}
          columns={[
            {
              label: t("Application / resource"),
              render: (r) => (
                <>
                  <strong>{r.name}</strong>
                  <small className="hosting-subline">
                    {r.kind} · {r.namespace || t("Cluster")}
                  </small>
                </>
              ),
            },
            {
              label: t("Status"),
              render: (r) => (
                <>
                  <span className={`hosting-state ${r.status}`}>
                    {t(uiLabel(r.status))}
                  </span>
                  {r.replicas !== undefined && (
                    <small className="hosting-subline">
                      {r.ready ?? 0} / {r.replicas} {t("Instances")}
                    </small>
                  )}
                </>
              ),
            },
            {
              label: t("Resources"),
              render: (r) => (
                <>
                  {r.cpuMillicores !== undefined && (
                    <small className="hosting-subline">
                      {r.kind === "Node"
                        ? t("Allocatable")
                        : t("Container requests")}
                    </small>
                  )}
                  {r.cpuMillicores !== undefined && (
                    <span>
                      {formatResource("cpuMillicores", r.cpuMillicores, locale)}{" "}
                      {t("CPU cores")}
                    </span>
                  )}
                  {r.memoryBytes !== undefined && (
                    <small className="hosting-subline">
                      {formatResource("memoryBytes", r.memoryBytes, locale)}{" "}
                      {t("Memory")}
                    </small>
                  )}
                  {r.storageBytes !== undefined && (
                    <small>
                      {formatResource("storageBytes", r.storageBytes, locale)}
                    </small>
                  )}
                  {r.usageCPU !== undefined && (
                    <small className="hosting-subline">
                      {t("Actual usage")}:{" "}
                      {formatResource("cpuMillicores", r.usageCPU, locale)} /{" "}
                      {formatResource(
                        "memoryBytes",
                        r.usageMemory ?? 0,
                        locale,
                      )}
                    </small>
                  )}
                </>
              ),
            },
            {
              label: t("Actions"),
              render: (r) =>
                inventory.canWrite && r.namespace && !inventory.stale ? (
                  <details>
                    <summary>{t("Manage")}</summary>
                    {r.controlled && (
                      <p>
                        {t(
                          "Another controller manages this application. Changes are blocked.",
                        )}
                      </p>
                    )}
                    {[
                      "Deployment",
                      "StatefulSet",
                      "DaemonSet",
                      "Pod",
                      "Job",
                    ].includes(r.kind) && (
                      <HostingForm
                        command={{
                          action: "byok.workload",
                          clusterID,
                          uid: r.uid,
                          resourceVersion: r.resourceVersion,
                          operation: "logs",
                          idempotencyKey: randomUUID(),
                        }}
                        label={t("Read logs")}
                      />
                    )}
                    {!r.controlled && (
                      <>
                        {["Deployment", "StatefulSet"].includes(r.kind) && (
                          <HostingForm
                            command={{
                              action: "byok.workload",
                              clusterID,
                              uid: r.uid,
                              resourceVersion: r.resourceVersion,
                              operation: "scale",
                              replicas: r.replicas ?? 1,
                              idempotencyKey: randomUUID(),
                            }}
                            label={t("Scale")}
                          >
                            <label className="field">
                              {t("Instances")}
                              <input
                                type="number"
                                name="replicas"
                                min="0"
                                max="100"
                                defaultValue={r.replicas ?? 1}
                                required
                              />
                            </label>
                          </HostingForm>
                        )}
                        {(["Deployment", "StatefulSet", "DaemonSet"].includes(
                          r.kind,
                        )
                          ? ["restart"]
                          : r.kind === "CronJob"
                            ? [r.suspended ? "resume" : "suspend"]
                            : []
                        ).map((operation) => (
                          <HostingForm
                            key={operation}
                            command={{
                              action: "byok.workload",
                              clusterID,
                              uid: r.uid,
                              resourceVersion: r.resourceVersion,
                              operation: operation as
                                "restart" | "resume" | "suspend",
                              idempotencyKey: randomUUID(),
                            }}
                            label={
                              operation === "restart"
                                ? "Restart"
                                : operation === "resume"
                                  ? "Resume"
                                  : "Suspend"
                            }
                          />
                        ))}
                        {["Deployment", "StatefulSet"].includes(r.kind) && (
                          <HostingForm
                            command={{
                              action: "byok.workload",
                              clusterID,
                              uid: r.uid,
                              resourceVersion: r.resourceVersion,
                              operation: r.replicas === 0 ? "start" : "stop",
                              idempotencyKey: randomUUID(),
                            }}
                            label={
                              r.replicas === 0 ? "Start application" : "Stop"
                            }
                          />
                        )}
                      </>
                    )}
                  </details>
                ) : null,
            },
          ]}
        />
        <nav className="hosting-pagination">
          {page > 1 && (
            <Link
              href={`${path}?${new URLSearchParams({ ...q, page: String(page - 1) })}`}
            >
              {t("Previous")}
            </Link>
          )}
          <span>
            {inventory.total} {t("Resources")}
          </span>
          {page * 50 < inventory.total && (
            <Link
              href={`${path}?${new URLSearchParams({ ...q, page: String(page + 1) })}`}
            >
              {t("Next")}
            </Link>
          )}
        </nav>
      </section>
      <details className="panel">
        <summary>{t("Connection settings")}</summary>
        <p>
          {c.region} · {c.locationEvidence}
        </p>
        <p>
          {t(
            "Disconnecting stops Webdock management. Applications keep running on your cluster.",
          )}
        </p>
        {c.canWrite && (
          <>
            <HostingForm
              command={{
                action: "byok.revoke",
                clusterID,
                revision: c.revision,
              }}
              label={t("Disconnect agent")}
            />
            <HostingForm
              command={{ action: "byok.enrollment", clusterID }}
              label={t("Create new enrollment")}
            />
          </>
        )}
      </details>
    </>
  );
}
