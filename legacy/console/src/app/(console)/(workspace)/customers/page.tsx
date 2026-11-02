import { searchPage } from "@webdock/search";
import { uiLabel } from "@/lib/ui-labels";

import { getRequestI18n } from "@webdock/i18n/next";
import Link from "next/link";
import { redirect } from "next/navigation";
import type { RegistryWhere as Where } from "@webdock/api-client/registry";
import { requireRegistryOperator } from "../../../../lib/registry-page";
import { date } from "../../../../lib/presentation";
import {
  listQuery,
  listURL,
  type SearchParams,
} from "../../../../lib/list-query";
import { ListControls, Pagination } from "../../../../components/list-controls";
export default async function Customers({
  searchParams,
}: {
  searchParams: Promise<SearchParams>;
}) {
  const i18n = await getRequestI18n();

  const { payload, user } = await requireRegistryOperator();
  const query = listQuery(await searchParams, "name");
  const where: Where = {
    and: [
      ...(query.status === "all" ? [] : [{ status: { equals: query.status } }]),
    ],
  };
  const find = (page: number, limit: number) =>
    payload.find({
      collection: "customers",
      where,
      page,
      limit,
      sort: [query.sort, "id"],
      overrideAccess: false,
      user,
    });
  const customers = query.q
    ? await searchPage(
        async (page) => {
          const result = await find(page, 300);
          return { rows: result.docs, hasMore: result.hasNextPage };
        },
        (c) =>
          [
            c.name,
            c.firstName,
            c.lastName,
            c.companyName,
            c.contactName,
            c.contactEmail,
          ].join(" "),
        query.q,
        query.page,
        20,
      )
    : await find(query.page, 20);

  if (query.page > Math.max(1, customers.totalPages))
    redirect(
      listURL("/customers", {
        ...query,
        page: Math.max(1, customers.totalPages),
      }),
    );
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>{i18n.t("Customers")}</h1>
          <p>
            {i18n.t(
              "Each customer has a tenant for their projects, with access by invitation.",
            )}
          </p>
        </div>
        <Link className="button" href="/customers/new">
          {i18n.t("New customer +")}
        </Link>
      </div>
      <ListControls
        path="/customers"
        q={query.q}
        status={query.status}
        sort={query.sort}
        placeholder={i18n.t("Name, contact or email")}
      />
      <div className="table-wrap">
        <table className="records-table" role="table">
          <caption className="sr-only">
            {i18n.t("Customers matching the selected filters")}
          </caption>
          <thead>
            <tr>
              <th scope="col">{i18n.t("Customer")}</th>
              <th scope="col">{i18n.t("Contact")}</th>
              <th scope="col">{i18n.t("Status")}</th>
              <th scope="col">{i18n.t("Updated")}</th>
              <th scope="col">
                <span className="sr-only">{i18n.t("Manage")}</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {customers.docs.map((c) => (
              <tr key={c.id}>
                <td data-label={i18n.t("Customer")}>
                  <Link className="project-name" href={"/customers/" + c.id}>
                    {c.name}
                  </Link>
                  <small>
                    {c.customerType === "person"
                      ? i18n.t("Person")
                      : i18n.t("Company")}
                    {i18n.t(" · Tenant")}
                  </small>
                </td>
                <td data-label={i18n.t("Contact")}>
                  {c.contactName || i18n.t("No contact person")}
                  <small>
                    {c.contactEmail ? (
                      <a href={"mailto:" + c.contactEmail}>{c.contactEmail}</a>
                    ) : (
                      "No email recorded"
                    )}
                  </small>
                </td>
                <td data-label={i18n.t("Status")}>
                  <span
                    className={
                      "badge " + (c.status === "active" ? "connected" : "")
                    }
                  >
                    {i18n.t(uiLabel(c.status))}
                  </span>
                </td>
                <td className="muted" data-label={i18n.t("Updated")}>
                  {date(c.updatedAt, i18n.locale)}
                </td>
                <td data-label={i18n.t("Manage")}>
                  <Link
                    className="row-link"
                    aria-label={i18n.t("Manage ") + c.name}
                    href={"/customers/" + c.id}
                  >
                    ↗
                  </Link>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {!customers.totalDocs && (
          <div className="empty">
            <h3>
              {query.q
                ? i18n.t("No matching customers")
                : query.status === "archived"
                  ? i18n.t("No archived customers")
                  : i18n.t("Your first customer starts here")}
            </h3>
            <p>
              {query.q
                ? i18n.t("Try another name, contact or email address.")
                : i18n.t(
                    "Customer records keep contact details and related projects together.",
                  )}
            </p>
            <Link
              className="button secondary"
              href={query.q ? "/customers" : "/customers/new"}
            >
              {query.q ? i18n.t("Reset filters") : i18n.t("Create a customer")}
            </Link>
          </div>
        )}
      </div>
      <Pagination
        path="/customers"
        query={query}
        page={query.page}
        totalPages={customers.totalPages}
        totalDocs={customers.totalDocs}
        limit={20}
      />
    </>
  );
}
