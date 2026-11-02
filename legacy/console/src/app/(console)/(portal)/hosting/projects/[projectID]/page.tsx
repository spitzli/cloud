import { applicationState } from "@/lib/workspace-navigation";
import { ListControls, Pagination } from "@/components/list-controls";
import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
import {
  type HostingAppView,
  type HostingPage,
} from "@webdock/hosting-contracts";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingTable } from "@/components/hosting/table";
import { HostingRefresh } from "@/components/hosting/refresh";
export default async function Applications({
  params,
  searchParams,
}: {
  params: Promise<{ projectID: string }>;
  searchParams: Promise<{ q?: string; page?: string; sort?: string }>;
}) {
  const { projectID } = await params;
  const query = await searchParams;
  const page = /^[1-9][0-9]{0,3}$/.test(query.page ?? "")
    ? Number(query.page)
    : 1;
  const search = (query.q ?? "").slice(0, 160),
    sort = query.sort === "-name" ? "-name" : "name";
  const { t } = await getRequestI18n();
  const data = await hostingPageCall<
    HostingPage<HostingAppView> & {
      canManage: boolean;
      ownImages: boolean;
      subscriptionRevision: number;
    }
  >(
    { action: "apps.list", projectID, page, limit: 20, search, sort },
    `/hosting/projects/${encodeURIComponent(projectID)}?${new URLSearchParams({ q: search, sort, page: String(page) })}`,
  );
  return (
    <>
      <div className="page-heading">
        <h1>{t("Applications")}</h1>
        {data.canManage && (
          <Link
            className="button"
            href={`/hosting/projects/${projectID}/setup`}
          >
            {t("Add application")}
          </Link>
        )}
      </div>
      <Link
        className="button secondary"
        href={`/hosting/projects/${projectID}/git`}
      >
        {t("Git deployments")}
      </Link>
      <HostingRefresh active={data.docs.some((a) => a.status === "pending")} />
      <section className="panel">
        <ListControls
          path={`/hosting/projects/${projectID}`}
          q={search}
          sort={sort}
          placeholder={t("Search")}
          sortOptions={[
            { value: "name", label: "Name A–Z" },
            { value: "-name", label: "Name Z–A" },
          ]}
        />
        <HostingTable
          rows={data.docs}
          rowKey={(a) => a.id}
          empty={t("No applications deployed yet.")}
          columns={[
            {
              label: t("Name"),
              render: (a) => (
                <Link href={`/hosting/apps/${a.id}`}>{a.name}</Link>
              ),
            },
            {
              label: t("Status"),
              render: (a) => t(applicationState(a.status)),
            },
            { label: t("Replicas"), render: (a) => a.spec.replicas },
          ]}
        />
        <Pagination
          path={`/hosting/projects/${projectID}`}
          query={{ q: search, sort }}
          {...data}
        />
      </section>
    </>
  );
}
