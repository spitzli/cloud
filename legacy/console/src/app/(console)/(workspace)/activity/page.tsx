import { searchPage } from "@webdock/search";
import { auditSummary, auditFieldLabel } from "@/lib/ui-labels";

import { getRequestI18n } from "@webdock/i18n/next";
import Link from "next/link";
import { redirect } from "next/navigation";
import type { RegistryWhere as Where } from "@webdock/api-client/registry";
import { requireRegistryOperator } from "../../../../lib/registry-page";
import {
  listQuery,
  listURL,
  queryText,
  type SearchParams,
} from "../../../../lib/list-query";
import { ListControls, Pagination } from "../../../../components/list-controls";
export default async function Activity({
  searchParams,
}: {
  searchParams: Promise<SearchParams>;
}) {
  const i18n = await getRequestI18n();

  const { payload, user } = await requireRegistryOperator();
  const params = await searchParams;
  const query = listQuery(params, "-createdAt");
  const sort = query.sort === "createdAt" ? "createdAt" : "-createdAt";
  const collection = ["projects", "customers", "cms-instances"].includes(
    queryText(params.collection),
  )
    ? queryText(params.collection)
    : "";
  const target = queryText(params.target);
  const action = ["create", "update"].includes(queryText(params.action))
    ? queryText(params.action)
    : "";
  const where: Where = {
    and: [
      ...(collection ? [{ targetCollection: { equals: collection } }] : []),
      ...(target ? [{ targetID: { equals: target } }] : []),
      ...(action ? [{ action: { equals: action } }] : []),
    ],
  };
  const find = (page: number, limit: number) =>
    payload.find({
      collection: "audit-events",
      where,
      page,
      limit,
      depth: 1,
      sort: [sort, "id"],
      overrideAccess: false,
      user,
    });
  const events = query.q
    ? await searchPage(
        async (page) => {
          const result = await find(page, 300);
          return { rows: result.docs, hasMore: result.hasNextPage };
        },
        (e) => [e.summary, e.changedFields].join(" "),
        query.q,
        query.page,
        30,
      )
    : await find(query.page, 30);
  const filters = { q: query.q, sort, collection, action, target };
  if (query.page > Math.max(1, events.totalPages))
    redirect(
      listURL("/activity", {
        ...filters,
        page: Math.max(1, events.totalPages),
      }),
    );
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>{i18n.t("Activity")}</h1>
          <p>
            {i18n.t("A lasting record of who changed what in your workspace.")}
          </p>
        </div>
      </div>
      <ListControls
        path="/activity"
        q={query.q}
        sort={sort}
        placeholder={i18n.t("Change summary or field")}
        activity
        extra={
          <>
            <div>
              <label htmlFor="collection">{i18n.t("Record type")}</label>
              <select
                id="collection"
                name="collection"
                defaultValue={collection}
               data-search-default="">
                <option value="">{i18n.t("All records")}</option>
                <option value="projects">{i18n.t("Projects")}</option>
                <option value="customers">{i18n.t("Customers")}</option>
                <option value="cms-instances">
                  {i18n.t("CMS connections")}
                </option>
              </select>
            </div>
            <div>
              <label htmlFor="action">{i18n.t("Change")}</label>
              <select id="action" name="action" defaultValue={action} data-search-default="">
                <option value="">{i18n.t("All changes")}</option>
                <option value="create">{i18n.t("Created")}</option>
                <option value="update">{i18n.t("Updated")}</option>
              </select>
            </div>
            {target && <input name="target" type="hidden" value={target} />}
          </>
        }
      />
      {target && (
        <p className="filter-context">
          {i18n.t("History for record ")}
          <code>{target}</code>.{" "}
          <Link href={listURL("/activity", { ...filters, target: "" })}>
            {i18n.t("Show all records")}
          </Link>
        </p>
      )}
      <section
        className="panel activity-list"
        aria-label={i18n.t("Recorded changes")}
      >
        {events.docs.map((e) => (
          <div className="activity-row" key={e.id}>
            <span className="activity-marker" aria-hidden="true">
              {e.action === "create" ? "+" : "↺"}
            </span>
            <div className="activity-content">
              <strong>{auditSummary(e.summary, i18n.t)}</strong>
              <small>
                {typeof e.actor === "object"
                  ? e.actor.name
                  : i18n.t("Operator")}
                {e.changedFields
                  ? " · " +
                    e.changedFields
                      .split(",")
                      .map((field) => i18n.t(auditFieldLabel(field)))
                      .join(", ")
                  : ""}
              </small>
              {["customers", "projects"].includes(e.targetCollection) && (
                <Link
                  className="activity-target"
                  href={"/" + e.targetCollection + "/" + e.targetID}
                >
                  {i18n.t("View")}{" "}
                  {e.targetCollection === "projects"
                    ? i18n.t("project")
                    : i18n.t("customer")}
                </Link>
              )}
            </div>
            <time dateTime={e.createdAt}>
              {new Intl.DateTimeFormat(
                i18n.locale === "de" ? "de-DE" : "en-GB",
                {
                  dateStyle: "medium",
                  timeStyle: "short",
                  timeZone: "UTC",
                },
              ).format(new Date(e.createdAt))}
              <small>{i18n.t("UTC")}</small>
            </time>
          </div>
        ))}
        {!events.totalDocs && (
          <div className="empty">
            <h2>
              {query.q || collection || action || target
                ? i18n.t("No matching changes")
                : i18n.t("No changes yet")}
            </h2>
            <p>
              {query.q || collection || action || target
                ? i18n.t("Broaden your filters to see more workspace activity.")
                : i18n.t(
                    "Customer, project and CMS updates will appear here with their author and time.",
                  )}
            </p>
            {(query.q || collection || action || target) && (
              <Link className="button secondary" href="/activity">
                {i18n.t("Reset filters")}
              </Link>
            )}
          </div>
        )}
      </section>
      <Pagination
        path="/activity"
        query={filters}
        page={query.page}
        totalPages={events.totalPages}
        totalDocs={events.totalDocs}
        limit={30}
      />
    </>
  );
}
