import { searchPage } from "@webdock/search";

import { getRequestI18n } from "@webdock/i18n/next";
import Link from "next/link";
import { redirect } from "next/navigation";
import type { RegistryWhere as Where } from "@webdock/api-client/registry";
import { requireRegistryOperator } from "../../../lib/registry-page";
import {
  relatedID,
  date,
  hostname,
  validRecordID,
} from "../../../lib/presentation";
import {
  listQuery,
  listURL,
  queryText,
  type SearchParams,
} from "../../../lib/list-query";
import { ListControls, Pagination } from "../../../components/list-controls";
export default async function Projects({
  searchParams,
}: {
  searchParams: Promise<SearchParams>;
}) {
  const i18n = await getRequestI18n();


  const { payload, user } = await requireRegistryOperator();
  const params = await searchParams;
  const query = listQuery(params);
  const rawCustomer = queryText(params.customer);
  const customer = validRecordID(rawCustomer) ? rawCustomer : "";
  const where: Where = {
    and: [
      ...(query.status === "all" ? [] : [{ status: { equals: query.status } }]),
      ...(customer ? [{ customer: { equals: customer } }] : []),
    ],
  };
  const find = (page: number, limit: number) =>
    payload.find({
      collection: "projects",
      where,
      page,
      limit,
      depth: 1,
      sort: [query.sort, "id"],
      user,
      overrideAccess: false,
    });
  const [projects, active, archived] = await Promise.all([
    query.q
      ? searchPage(
          async (page) => {
            const result = await find(page, 300);
            return { rows: result.docs, hasMore: result.hasNextPage };
          },
          (p) =>
            [
              p.name,
              p.url,
              typeof p.customer === "object" && p.customer
                ? p.customer.name
                : "",
            ].join(" "),
          query.q,
          query.page,
          20,
        )
      : find(query.page, 20),
    payload.count({
      collection: "projects",
      where: { status: { equals: "active" } },
      user,
      overrideAccess: false,
    }),
    payload.count({
      collection: "projects",
      where: { status: { equals: "archived" } },
      user,
      overrideAccess: false,
    }),

  ]);
  if (query.page > Math.max(1, projects.totalPages))
    redirect(
      listURL("/", {
        ...query,
        customer,
        page: Math.max(1, projects.totalPages),
      }),
    );
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>{i18n.t("Projects")}</h1>
          <p>
            {i18n.t("Websites, customers and content systems, in one place.")}
          </p>
        </div>
        <Link className="button" href="/projects/new">
          {i18n.t("New project +")}
        </Link>
      </div>
      <div className="overview-line">
        <Link href="/?status=active">
          <strong>{active.totalDocs}</strong>
          {i18n.t(" Active projects")}
        </Link>
        <Link href="/?status=archived">
          <strong>{archived.totalDocs}</strong>
          {i18n.t(" Archived projects")}
        </Link>

      </div>
      <ListControls
        path="/"
        q={query.q}
        status={query.status}
        sort={query.sort}
        placeholder={i18n.t("Project, customer or domain")}
        extra={
          customer ? (
            <input type="hidden" name="customer" value={customer} />
          ) : undefined
        }
      />
      {customer && (
        <p className="filter-context">
          {i18n.t("Showing projects for one customer.")}{" "}
          <Link href={listURL("/", { ...query, page: 1 })}>
            {i18n.t("Show all customers")}
          </Link>
        </p>
      )}
      <div className="table-wrap">
        <table className="records-table" role="table">
          <caption className="sr-only">
            {i18n.t("Projects matching the selected filters")}
          </caption>
          <thead>
            <tr>
              <th scope="col">{i18n.t("Project")}</th>
              <th scope="col">{i18n.t("Customer")}</th>
              <th scope="col">{i18n.t("Updated")}</th>
              <th scope="col">
                <span className="sr-only">{i18n.t("Manage")}</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {projects.docs.map((project) => (
              <tr key={project.id}>
                <td data-label={i18n.t("Project")}>
                  <Link
                    className="project-name"
                    href={"/projects/" + project.id}
                  >
                    {project.name}
                  </Link>
                  <small>
                    {hostname(project.url, i18n.t("No domain yet"))}
                    {project.status === "archived" && i18n.t(" · Archived")}
                  </small>
                </td>
                <td data-label={i18n.t("Customer")}>
                  <Link href={"/customers/" + relatedID(project.customer)}>
                    {typeof project.customer === "object"
                      ? project.customer.name
                      : i18n.t("Customer")}
                  </Link>
                </td>

                <td className="muted" data-label={i18n.t("Updated")}>
                  <time dateTime={project.updatedAt}>
                    {date(project.updatedAt, i18n.locale)}
                  </time>
                </td>
                <td data-label={i18n.t("Manage")}>
                  <Link
                    className="row-link"
                    href={"/projects/" + project.id}
                    aria-label={i18n.t("Manage ") + project.name}
                  >
                    ↗
                  </Link>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {!projects.totalDocs && (
          <div className="empty">
            <h3>
              {query.q || customer
                ? i18n.t("No matching projects")
                : query.status === "archived"
                  ? i18n.t("No archived projects")
                  : i18n.t("Make room for your next project")}
            </h3>
            <p>
              {query.q || customer
                ? i18n.t(
                    "Try a different name or domain, or reset your filters.",
                  )
                : i18n.t(
                    "Add a customer and a project to keep its website and CMS details together.",
                  )}
            </p>
            <Link
              className="button secondary"
              href={query.q || customer ? "/" : "/projects/new"}
            >
              {query.q || customer
                ? i18n.t("Reset filters")
                : i18n.t("Create a project")}
            </Link>
          </div>
        )}
      </div>
      <Pagination
        path="/"
        query={{ ...query, customer }}
        page={query.page}
        totalPages={projects.totalPages}
        totalDocs={projects.totalDocs}
        limit={20}
      />

    </>
  );
}
