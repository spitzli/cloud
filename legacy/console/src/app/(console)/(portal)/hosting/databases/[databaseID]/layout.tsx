import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
import { getManagedDatabasePage } from "@/lib/managed-database-page";
import { HostingProjectHeader } from "@/components/hosting/project-context";
import { HostingRefresh } from "@/components/hosting/refresh";

export default async function DatabaseLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ databaseID: string }>;
}) {
  const { databaseID } = await params,
    { t } = await getRequestI18n();
  const {
    database: d,
    path,
  } = await getManagedDatabasePage(databaseID);
  return (
    <>
      <HostingProjectHeader
        projectID={d.projectID}
        leaf={{ label: d.name, href: path }}
        showNavigation={false}
      />
      <div className="page-heading">
        <div>
          <h1>{d.name}</h1>
          <p className="muted">
            {d.kind === "shared" ? t("Shared") : t("Dedicated instance")}
          </p>
        </div>
        <Link href={`/hosting/projects/${d.projectID}/databases`}>
          {t("All project databases")}
        </Link>
      </div>
      <HostingRefresh active={d.state !== "purged"} interval={75000} />
      {children}
    </>
  );
}
