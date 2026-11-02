import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingTable } from "@/components/hosting/table";
import { getStudioSession } from "@/lib/studio-client";
import { databaseCall } from "@/lib/database-client";
import { withHostingPageAccess } from "@/lib/hosting-page-access";
import type { DatabaseDirectory } from "@webdock/database-contracts";
import { ManagedDatabaseRows } from "@/components/databases/managed-rows";
import "../databases/style.css";
export default async function ManagedDatabases({
  params,
}: {
  params: Promise<{ customerID: string }>;
}) {
  const { customerID } = await params;
  const { t } = await getRequestI18n();
  const session = await getStudioSession();
  const directory = !session?.preview ? await withHostingPageAccess(
    `/tenants/${customerID}/managed-databases`,
    () => databaseCall<DatabaseDirectory>({ action: "list", customerID }),
  ) : null;
  const projects = await hostingPageCall<
    Array<{ id: string; name: string; count: number }>
  >(
    { action: "databases.projects.list", customerID },
    `/tenants/${customerID}/managed-databases`,
  );
  return (
    <>
      <h1>{t("Databases")}</h1>
      {!!directory?.managed.length && <section className="panel database-directory">
        <ManagedDatabaseRows databases={directory.managed} />
      </section>}
      <p>{t("Choose a project to order and manage PostgreSQL.")}</p>
      <section className="panel">
        <HostingTable
          rows={projects}
          rowKey={(p) => p.id}
          empty={t("No active projects.")}
          columns={[
            {
              label: t("Project"),
              render: (p) => (
                <Link href={`/hosting/projects/${p.id}/databases`}>
                  {p.name}
                </Link>
              ),
            },
            { label: t("Databases"), render: (p) => p.count },
          ]}
        />
      </section>
    </>
  );
}
