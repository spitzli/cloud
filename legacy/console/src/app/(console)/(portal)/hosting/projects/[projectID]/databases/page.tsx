import { randomUUID } from "node:crypto";
import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
import type { DatabaseView } from "@webdock/hosting-contracts";
import { hostingPageCall } from "@/lib/hosting-client";
import {
  DatabaseOrderForm,
  type DatabaseCatalogEntry,
} from "@/components/hosting/databases";
import { HostingTable } from "@/components/hosting/table";
import { HostingRefresh } from "@/components/hosting/refresh";
export default async function Databases({
  params,
}: {
  params: Promise<{ projectID: string }>;
}) {
  const { projectID } = await params;
  const path = `/hosting/projects/${projectID}/databases`;
  const { t } = await getRequestI18n();
  const [rows, plans, access] = await Promise.all([
    hostingPageCall<DatabaseView[]>(
      { action: "databases.list", projectID },
      path,
    ),
    hostingPageCall<DatabaseCatalogEntry[]>(
      { action: "databases.plans.list", projectID },
      path,
    ),
    hostingPageCall<{ canWrite: boolean }>(
      { action: "databases.permissions", projectID },
      path,
    ),
  ]);
  return (
    <>
      <h1>{t("Managed PostgreSQL")}</h1>
      <HostingRefresh
        interval={75000}
        active={rows.some((d) =>
          [
            "provisioning",
            "pausing",
            "resuming",
            "updating",
            "deleting",
          ].includes(d.state),
        )}
      />
      <section className="panel">
        <HostingTable
          rows={rows}
          rowKey={(r) => r.id}
          empty={t("No databases ordered.")}
          columns={[
            {
              label: t("Name"),
              render: (r) => (
                <Link href={`/hosting/databases/${r.id}`}>{r.name}</Link>
              ),
            },
            {
              label: t("Database kind"),
              render: (r) =>
                r.kind === "shared" ? t("Shared") : t("Dedicated instance"),
            },
            { label: t("Status"), render: (r) => t(r.state) },
            { label: t("Backups"), render: () => t("Not configured") },
          ]}
        />
      </section>
      {access.canWrite && (
        <details className="workspace-disclosure" open={rows.length === 0}>
          <summary>{t("Add database")}</summary>
          <DatabaseOrderForm
            projectID={projectID}
            plans={plans.filter((p) => p.availability !== "disabled")}
            requestKey={randomUUID()}
          />
        </details>
      )}
    </>
  );
}
