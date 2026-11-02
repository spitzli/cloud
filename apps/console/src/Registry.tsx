import { useI18n } from "@webdock/i18n/react";
import { CustomerScopeStart } from "./CustomerScope";
import { CustomerSelfService } from "./plugins/CustomerSelfService";
import { CustomerPlan } from "./Usage";
import { CustomerMembers } from "./CustomerMembers";
import { formText } from "./form";
import { useEffect, useState } from "react";
import { CustomerFields, customerFields } from "./CustomerFields";
import { FieldError, useLiveValidation } from "./live-validation";
import { Link, useBlocker, useNavigate, useRouterState } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeftIcon } from "@phosphor-icons/react/dist/csr/ArrowLeft";
import { ArrowRightIcon } from "@phosphor-icons/react/dist/csr/ArrowRight";
import { PlusIcon } from "@phosphor-icons/react/dist/csr/Plus";
import { MagnifyingGlassIcon } from "@phosphor-icons/react/dist/csr/MagnifyingGlass";
import {
  listSchema,
  recordSchema,
  type Collection,
  type RegistryRecord,
} from "./api";
import { z } from "zod";
import { useApi } from "./context";
import { Empty, Failure, Loading, Status } from "./common";
import * as ui from "./ui";
const names = {
  customers: "Customers",
  projects: "Projects",
  "audit-events": "Audit trail",
};
export function isCollection(value: string): value is Collection {
  return Object.hasOwn(names, value);
}
const path = (collection: Collection, id?: string) =>
  `/v1/operator/registry/${collection}${id ? `/${encodeURIComponent(id)}` : ""}`;
const displayDate = (value?: string) =>
  value ? new Date(value).toLocaleString() : "—";

export function RegistryList({
  collection,
  customer,
}: {
  collection: Collection;
  customer?: string | undefined;
}) {
  const api = useApi();
  const [search, setSearch] = useState("");
  const [committedSearch, setCommittedSearch] = useState("");
  const [page, setPage] = useState(1);
  const [status, setStatus] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => {
      setCommittedSearch(search);
      setPage(1);
    }, 250);
    return () => {
      clearTimeout(timer);
    };
  }, [search]);
  const query = new URLSearchParams({
    page: String(page),
    limit: "20",
    sort: collection === "audit-events" ? "-createdAt" : "-updatedAt",
    ...(committedSearch ? { search: committedSearch } : {}),
    ...(customer ? { customer } : {}),
    ...(status ? { status } : {}),
  });
  const records = useQuery({
    queryKey: ["registry", collection, query.toString()],
    queryFn: ({ signal }) =>
      api.get(`${path(collection)}?${query}`, listSchema, signal),
  });
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>{names[collection]}</h1>
          <p className={ui.pageLead}>
            {collection === "customers"
              ? "Manage customer records and their projects."
              : collection === "projects"
                ? "Choose a project to manage its workspace."
                : "Review recorded changes across the installation."}
          </p>
        </div>
        {collection !== "audit-events" ? (
          <Link
            className={ui.button("primary")}
            to="/$collection/new"
            params={{ collection }}
            search={{ customer }}
          >
            <PlusIcon />
            New {collection === "customers" ? "customer" : "project"}
          </Link>
        ) : null}
      </div>
      <div className="mb-4 flex flex-wrap items-center gap-2 text-[13px]">
        <label className="relative w-full sm:w-[340px]">
          <MagnifyingGlassIcon aria-hidden size={16} className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-muted" />
          <input
            className={ui.cx(`${ui.input} min-h-9 pl-9`)}
            aria-label={`Search ${names[collection].toLowerCase()}`}
            placeholder={`Search ${names[collection].toLowerCase()}…`}
            value={search}
            maxLength={160}
            onChange={(event) => {
              setSearch(event.target.value);
            }}
          />
        </label>
        {collection !== "audit-events" ? (
          <select
            className={ui.cx(`${ui.input} min-h-9 w-auto max-sm:flex-1`)}
            aria-label="Filter by record status"
            value={status}
            onChange={(event) => {
              setStatus(event.target.value);
              setPage(1);
            }}
          >
            <option value="">All statuses</option>
            <option value="active">Active</option>
            <option value="archived">Archived</option>
          </select>
        ) : null}
        {records.data ? (
          <span className="ml-auto text-muted">
            {records.data.totalDocs}{" "}
            {records.data.totalDocs === 1 ? "record" : "records"}
          </span>
        ) : null}
      </div>
      {records.isPending ? (
        <Loading>Loading {names[collection].toLowerCase()}…</Loading>
      ) : records.isError ? (
        <Failure
          error={records.error}
          retry={() => {
            void records.refetch();
          }}
        />
      ) : records.data.docs.length === 0 ? (
        <Empty
          title={
            committedSearch
              ? "No matching records"
              : `No ${names[collection].toLowerCase()} yet`
          }
        >
          {committedSearch
            ? "Try a different search."
            : collection === "audit-events"
              ? "Recorded changes will appear here."
              : "Create the first record to get started."}
        </Empty>
      ) : (
        <>
          <div className={ui.tableFrame}>
            <table className={ui.table}>
              <thead>
                <tr>
                  <th scope="col" className={ui.cx(`${ui.th} w-1/2`)}>
                    {collection === "audit-events" ? "Change" : "Name"}
                  </th>
                  <th scope="col" className={ui.th}>
                    {collection === "audit-events" ? "Target" : "Status"}
                  </th>
                  <th scope="col" className={ui.th}>
                    {collection === "audit-events" ? "Recorded" : "Updated"}
                  </th>
                </tr>
              </thead>
              <tbody>
                {records.data.docs.map((record) => (
                  <tr key={record.id} className={ui.tr}>
                    <td className={ui.cx(`${ui.td} min-w-[170px]`)}>
                      <Link
                        to="/$collection/$id"
                        params={{ collection, id: record.id }}
                        className={ui.cx(`inline-block py-0.5 font-medium ${ui.rowLink}`)}
                      >
                        {record.name ??
                          record.summary ??
                          record.action ??
                          record.id}
                      </Link>
                      <span className={ui.cx(`block text-muted ${ui.mono}`)}>
                        {collection === "customers"
                          ? (record.contactEmail ?? record.id)
                          : record.id}
                      </span>
                    </td>
                    <td className={ui.td}>
                      {collection === "audit-events" ? (
                        (record.targetCollection ?? "—")
                      ) : (
                        <Status status={record.status} />
                      )}
                    </td>
                    <td className={ui.cx(`${ui.td} whitespace-nowrap text-muted`)}>
                      {displayDate(
                        collection === "audit-events"
                          ? record.createdAt
                          : record.updatedAt,
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="flex flex-wrap items-center justify-between gap-3 py-3.5 text-[13px]">
            <span className="text-muted">
              Page {records.data.page} of {records.data.totalPages}
              {records.isFetching ? " · Updating…" : ""}
            </span>
            <div className="flex gap-2">
              <button
                className={ui.button("quiet")}
                aria-label="Previous page"
                disabled={!records.data.hasPrevPage}
                onClick={() => {
                  setPage(page - 1);
                }}
              >
                <ArrowLeftIcon />
                Previous
              </button>
              <button
                className={ui.button("quiet")}
                disabled={!records.data.hasNextPage}
                onClick={() => {
                  setPage(page + 1);
                }}
              >
                Next
                <ArrowRightIcon />
              </button>
            </div>
          </div>
        </>
      )}
    </section>
  );
}
export function RegistryDetail({
  collection,
  id,
}: {
  collection: Collection;
  id: string;
}) {
  const api = useApi();
  const { t } = useI18n();
  const created = useRouterState({ select: (state) => state.location.state.created });
  const navigate = useNavigate();
  const record = useQuery({
    queryKey: ["registry", collection, id],
    queryFn: ({ signal }) =>
      api.get(path(collection, id), recordSchema, signal),
  });
  if (record.isPending) return <Loading>Loading record…</Loading>;
  if (record.isError)
    return (
      <Failure
        error={record.error}
        retry={() => {
          void record.refetch();
        }}
      />
    );
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <Link className={ui.backLink} to="/$collection" params={{ collection }}>
            <ArrowLeftIcon />
            {names[collection]}
          </Link>
          <h1 className={ui.pageTitle}>{record.data.name ?? record.data.summary ?? "Audit event"}</h1>
          <p className={ui.cx(`mt-1 text-muted ${ui.mono}`)}>{record.data.id}</p>
        </div>
        {collection !== "audit-events" ? (
          <Status status={record.data.status} />
        ) : null}
      </div>
      {collection === "customers" && record.data.status === "active" ? <CustomerScopeStart customer={id} /> : null}
      {created === "projects" && collection === "projects" ? (
        <p role="status" className={ui.cx(`${ui.notice} mb-6`)}>Project created.</p>
      ) : null}
      {created === "customers" && collection === "customers" ? (
        // Second step of creating a customer; leaving it (Done or Skip) drops the step from history.
        <CustomerMembers
          key={`members:${id}`}
          customer={id}
          welcome={{
            email: record.data.contactEmail ?? "",
            name: record.data.contactName ?? ([record.data.firstName, record.data.lastName].filter(Boolean).join(" ") || ""),
            done: () => { void navigate({ to: "/$collection/$id", params: { collection, id }, replace: true, state: {} }); },
          }}
        />
      ) : null}
      {collection === "customers" ? (
        <div className="mb-8 flex flex-wrap items-center gap-2 [&>a]:flex [&>a]:items-center [&>a]:gap-2 [&>a]:rounded-full [&>a]:border [&>a]:border-outline-strong [&>a]:px-3 [&>a]:py-1 [&>a]:text-[13px] [&>a]:font-medium [&>a:hover]:bg-soft [&>a:hover]:no-underline">
          <Link
            to="/$collection"
            params={{ collection: "projects" }}
            search={{ customer: id }}
          >
            View customer projects
            <ArrowRightIcon />
          </Link>
          <Link to="/workspaces/$id/connections" params={{ id }}>
            {t("Customer connections")}
            <ArrowRightIcon />
          </Link>
          {api.config.enabledFeatures.includes("hosting") ? (
            <Link to="/hosting" search={{ customer: id }}>
              Customer applications
              <ArrowRightIcon />
            </Link>
          ) : null}
        </div>
      ) : null}
      {collection === "projects" && record.data.customer ? (
        <div className="mb-8 flex flex-wrap items-center gap-2 [&>a]:flex [&>a]:items-center [&>a]:gap-2 [&>a]:rounded-full [&>a]:border [&>a]:border-outline-strong [&>a]:px-3 [&>a]:py-1 [&>a]:text-[13px] [&>a]:font-medium [&>a:hover]:bg-soft [&>a:hover]:no-underline">
          <Link
            to="/$collection/$id"
            params={{ collection: "customers", id: record.data.customer }}
          >
            Customer workspace
            <ArrowRightIcon />
          </Link>
          {api.config.enabledFeatures.includes("hosting") ? (
            <Link to="/hosting/projects/$projectID" params={{ projectID: id }}>
              Manage applications
              <ArrowRightIcon />
            </Link>
          ) : null}
        </div>
      ) : null}
      {collection === "audit-events" ? (
        <dl className="grid gap-4 sm:grid-cols-2">
          {[
            ["Action", record.data.action],
            ["Actor", record.data.actor],
            ["Changed fields", record.data.changedFields],
            ["Target", record.data.targetCollection],
            ["Record", record.data.targetID],
            ["Recorded", displayDate(record.data.createdAt)],
          ].map(([label, value]) => (
            <div key={label}>
              <dt className="text-xs text-muted">{label}</dt>
              <dd className="mt-1 text-sm">{value ?? "—"}</dd>
            </div>
          ))}
        </dl>
      ) : (
        <RegistryForm key={id} collection={collection} record={record.data} />
      )}
      {collection === "projects" && record.data.customer ? (
        <p className="mt-6">
          <Link className={ui.button()} to="/workspaces/$customer/projects/$id" params={{ customer: record.data.customer, id }}>
            Open project resources
          </Link>
        </p>
      ) : null}
      {collection === "customers" && record.data.status === "active" && created !== "customers" ? <CustomerMembers key={`members:${id}`} customer={id} /> : null}
      {collection === "customers" && record.data.status === "active" ? <CustomerSelfService customer={id} /> : null}
      {collection === "customers" ? <CustomerPlan customer={id} /> : null}
    </section>
  );
}
export function RegistryNew({
  collection,
  customer,
}: {
  collection: "customers" | "projects";
  customer?: string | undefined;
}) {
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <Link className={ui.backLink} to="/$collection" params={{ collection }}>
            <ArrowLeftIcon />
            {names[collection]}
          </Link>
          <h1 className={ui.pageTitle}>New {collection === "customers" ? "customer" : "project"}</h1>
          <p className={ui.pageLead}>
            {collection === "projects"
              ? "Projects belong to one customer workspace."
              : "Add the customer’s name and contact details. Next, you invite the first person."}
          </p>
        </div>
      </div>
      <RegistryForm collection={collection} customer={customer} />
    </section>
  );
}
type RegistryInput = Pick<
  RegistryRecord,
  | "name"
  | "notes"
  | "customerType"
  | "contactEmail"
  | "phone"
  | "customer"
  | "url"
  | "repositoryURL"
  | (typeof customerFields)[number]["key"]
>;
function RegistryForm({
  collection,
  record,
  customer,
}: {
  collection: "customers" | "projects";
  record?: RegistryRecord;
  customer?: string | undefined;
}) {
  const api = useApi();
  const client = useQueryClient();
  const [dirty, setDirty] = useState(false);
  const [saved, setSaved] = useState<RegistryRecord | null>(null);
  const [createdId, setCreatedId] = useState<string>();
  const [customerSearch, setCustomerSearch] = useState("");
  const [customerPage, setCustomerPage] = useState(1);
  const live = useLiveValidation();
  const navigate = useNavigate();
  const [selectedCustomer, setSelectedCustomer] = useState(
    record?.customer ?? customer ?? "",
  );
  const customers = useQuery({
    queryKey: ["registry", "customers", "select", customerSearch, customerPage],
    queryFn: ({ signal }) =>
      api.get(
        `${path("customers")}?status=active&limit=50&page=${String(customerPage)}&search=${encodeURIComponent(customerSearch)}`,
        listSchema,
        signal,
      ),
    enabled: collection === "projects" && !record,
  });
  useBlocker({
    shouldBlockFn: () =>
      dirty && !window.confirm("Leave this page and discard unsaved changes?"),
    enableBeforeUnload: dirty,
  });
  const write = useMutation({
    mutationFn: (input: RegistryInput) =>
      api.write(
        path(collection, record?.id ?? createdId),
        record || createdId ? "PUT" : "POST",
        input,
        recordSchema,
      ),
    onSuccess: (result) => {
      setDirty(false);
      setSaved(result);
      setCreatedId(result.id);
      void client.invalidateQueries({ queryKey: ["registry"] });
    },
  });
  // A new record opens its own page once the unsaved-changes guard has been released.
  useEffect(() => {
    if (!record && createdId && !dirty)
      void navigate({ to: "/$collection/$id", params: { collection, id: createdId }, state: { created: collection } });
  }, [record, createdId, dirty, collection, navigate]);

  return (
    <div className="grid items-start gap-8 lg:grid-cols-[minmax(0,1fr)_300px]">
      <form
        className={ui.card}
        {...live.form}
        onChange={() => {
          setDirty(true);
          setSaved(null);
        }}
        onSubmit={(event) => {
          event.preventDefault();
          const data = new FormData(event.currentTarget);
          const input: RegistryInput = {
            name: formText(data, "name").trim(),
            notes: formText(data, "notes").trim() || null,
          };
          if (collection === "customers") {
            for (const field of customerFields)
              input[field.key] = formText(data, field.key).trim() || null;
            input.customerType = formText(data, "customerType");
            input.contactEmail = formText(data, "contactEmail").trim() || null;
            input.phone = formText(data, "phone").trim() || null;
          } else {
            input.customer = record?.customer ?? selectedCustomer;
            input.url = formText(data, "url").trim() || null;
            input.repositoryURL =
              formText(data, "repositoryURL").trim() || null;
          }
          write.mutate(input);
        }}
      >
        <div className={ui.cx(`${ui.cardBody} gap-5`)}>
        <h2 className={ui.sectionTitle}>
          {collection === "customers" ? "Customer details" : "Project details"}
        </h2>
        <label className={ui.field}>
          Name
          <input
            className={ui.input}
            name="name"
            required
            maxLength={160}
            defaultValue={record?.name}
            autoComplete="organization"
            disabled={write.isPending}
          />
        </label>
        {live.errors.name ? <FieldError message={live.errors.name} /> : <span className={ui.cx(`${ui.hint} -mt-3`)}>How this {collection === "customers" ? "customer" : "project"} appears across Spitzli Cloud.</span>}
        {collection === "customers" ? (
          <CustomerFields record={record} pending={write.isPending} errors={live.errors} />
        ) : (
          <>
            {record ? (
              <label className={ui.field}>
                Customer
                <input className={ui.input} readOnly value={record.customer ?? ""} />
                <span className={ui.hint}>
                  Project ownership cannot be changed here.
                </span>
              </label>
            ) : (
              <fieldset className="grid min-w-0 border-0 p-0 gap-3">
                <legend className="px-0 pb-2 text-sm font-semibold">Customer workspace</legend>
                <label className={ui.field}>
                  Find a customer
                  <input
                    className={ui.input}
                    value={customerSearch}
                    maxLength={160}
                    onChange={(event) => {
                      setCustomerSearch(event.target.value);
                      setCustomerPage(1);
                    }}
                  />
                </label>
                {customers.isPending ? (
                  <Loading>Loading customers…</Loading>
                ) : customers.isError ? (
                  <Failure
                    error={customers.error}
                    retry={() => {
                      void customers.refetch();
                    }}
                  />
                ) : (
                  <>
                    <label className={ui.field}>
                      Customer
                      <select
                        className={ui.input}
                        name="customer"
                        required
                        value={selectedCustomer}
                        disabled={write.isPending}
                        onChange={(event) => {
                          setSelectedCustomer(event.target.value);
                        }}
                      >
                        <option value="">Choose an active customer</option>
                        {selectedCustomer &&
                        !customers.data.docs.some(
                          (row) => row.id === selectedCustomer,
                        ) ? (
                          <option value={selectedCustomer}>
                            {selectedCustomer}
                          </option>
                        ) : null}
                        {customers.data.docs.map((row) => (
                          <option key={row.id} value={row.id}>
                            {row.name}
                          </option>
                        ))}
                      </select>
                    </label>
                    <div className="flex gap-2">
                      <button
                        type="button"
                        className={ui.button("quiet")}
                        disabled={!customers.data.hasPrevPage}
                        onClick={() => {
                          setCustomerPage(customerPage - 1);
                        }}
                      >
                        Previous customers
                      </button>
                      <button
                        type="button"
                        className={ui.button("quiet")}
                        disabled={!customers.data.hasNextPage}
                        onClick={() => {
                          setCustomerPage(customerPage + 1);
                        }}
                      >
                        More customers
                      </button>
                    </div>
                  </>
                )}
              </fieldset>
            )}
            <label className={ui.field}>
              Website URL
              <input
                className={ui.input}
                name="url"
                type="url"
                pattern="https://.*"
                maxLength={2048}
                placeholder="https://"
                defaultValue={record?.url ?? ""}
                disabled={write.isPending}
              />
            </label>
            <label className={ui.field}>
              Repository URL
              <input
                className={ui.input}
                name="repositoryURL"
                type="url"
                pattern="https://.*"
                maxLength={2048}
                placeholder="https://"
                defaultValue={record?.repositoryURL ?? ""}
                disabled={write.isPending}
              />
            </label>
          </>
        )}
        <label className={ui.field}>
          Notes
          <textarea
            className={ui.cx(`${ui.input} resize-y`)}
            name="notes"
            rows={4}
            maxLength={3000}
            defaultValue={record?.notes ?? ""}
            disabled={write.isPending}
          />
        </label>
        {write.isError ? (
          <p role="alert" className="text-sm text-danger">
            {write.error.message}
          </p>
        ) : null}
        {saved ? (
          <p role="status" className="text-sm text-success">
            Saved.{" "}
            <Link to="/$collection/$id" params={{ collection, id: saved.id }}>
              Open record
            </Link>
          </p>
        ) : null}
        </div>
        <div className={ui.cx(`${ui.cardFooter} flex-row-reverse`)}>
          <button
            className={ui.button("primary")}
            disabled={
              write.isPending ||
              Boolean(record && !dirty) ||
              (collection === "projects" && !selectedCustomer)
            }
          >
            {write.isPending
              ? "Saving…"
              : record || createdId
                ? "Save changes"
                : `Create ${collection === "customers" ? "customer" : "project"}`}
          </button>
          {dirty ? <span>Unsaved changes</span> : null}
        </div>
      </form>
      <RecordInformation
        collection={collection}
        record={record}
        dirty={dirty}
      />
    </div>
  );
}

function RecordInformation({
  collection,
  record,
  dirty,
}: {
  collection: "customers" | "projects";
  record: RegistryRecord | undefined;
  dirty: boolean;
}) {
  const api = useApi();
  const client = useQueryClient();
  const navigate = useNavigate();
  const [archiveConfirmed, setArchiveConfirmed] = useState(false);
  const [deleteName, setDeleteName] = useState("");
  const remove = useMutation({
    mutationFn: () => {
      if (!record) throw Error("Save the record before deleting it.");
      return api.request(path(collection, record.id), z.object({ deleted: z.literal(true) }), { method: "DELETE" });
    },
    onSuccess: () => {
      if (record) client.removeQueries({ queryKey: ["registry", collection, record.id] });
      void client.invalidateQueries({ queryKey: ["registry"] });
      void navigate({ to: "/$collection", params: { collection } });
    },
  });
  const archive = useMutation({
    mutationFn: () => {
      if (!record) throw Error("Save the record before changing its status.");
      return api.write(
        path(collection, record.id),
        "PATCH",
        { archived: record.status !== "archived" },
        recordSchema,
      );
    },
    onSuccess: () => {
      setArchiveConfirmed(false);
      void client.invalidateQueries({ queryKey: ["registry"] });
    },
  });
  return (
    <aside className="grid gap-4 text-[13px] [overflow-wrap:anywhere]">
      <h2 className="text-sm font-semibold">Record information</h2>
      <p className="text-muted">
        {record
          ? `Created ${displayDate(record.createdAt)}`
          : "The record will be created when you save."}
      </p>
      {record ? (
        <>
          <p className="text-muted">Updated {displayDate(record.updatedAt)}</p>
          <div className={ui.cx(`${ui.card} mt-2 grid gap-2.5 p-5`)}>
            <h3 className="text-sm font-semibold">
              {record.status === "archived"
                ? "Restore record"
                : "Archive record"}
            </h3>
            <p className="text-muted">
              {record.status === "archived"
                ? "Make this record active again."
                : "Linked resources can prevent archiving. Stored data is retained."}
            </p>
            <label className="flex items-center gap-2 py-1.5">
              <input
                type="checkbox"
                className="size-4 accent-accent"
                checked={archiveConfirmed}
                onChange={(event) => {
                  setArchiveConfirmed(event.target.checked);
                }}
              />
              Confirm {record.status === "archived" ? "restore" : "archive"}
            </label>
            <button
              className={ui.cx(`${ui.button()} justify-self-start`)}
              disabled={!archiveConfirmed || archive.isPending || dirty}
              onClick={() => {
                archive.mutate();
              }}
            >
              {archive.isPending
                ? "Updating…"
                : record.status === "archived"
                  ? "Restore record"
                  : "Archive record"}
            </button>
            {archive.isError ? (
              <p role="alert" className="text-danger">
                {archive.error.message}
              </p>
            ) : null}
            {archive.isSuccess ? (
              <p role="status" className="text-success">
                Record status updated.
              </p>
            ) : null}
          </div>
          {collection === "customers" ? (
            <form
              aria-label="Delete customer"
              className={ui.cx(`${ui.card} grid gap-2.5 border-danger/40 p-5`)}
              onSubmit={(event) => {
                event.preventDefault();
                remove.mutate();
              }}
            >
              <h3 className="text-sm font-semibold text-danger">Delete customer</h3>
              <p className="text-muted">
                Removes this customer, its workspace members and pending invitations for good. Only possible without projects, hosting, billing, mail or database records.
              </p>
              {record.status !== "archived" ? <p className="font-medium">Archive this customer first.</p> : null}
              <label className={ui.field}>
                Type {record.name} to confirm
                <input className={ui.input} value={deleteName} autoComplete="off" disabled={record.status !== "archived"} onChange={(event) => { setDeleteName(event.target.value); }} />
              </label>
              <button
                className={ui.cx(`${ui.button("danger")} justify-self-start`)}
                disabled={record.status !== "archived" || deleteName !== record.name || remove.isPending || dirty}
              >
                {remove.isPending ? "Deleting…" : "Delete customer"}
              </button>
              {remove.isError ? (
                <p role="alert" className="text-danger">
                  {remove.error.message}
                </p>
              ) : null}
            </form>
          ) : null}
        </>
      ) : (
        <p className="text-muted">Access and changes are checked by the server.</p>
      )}
    </aside>
  );
}

declare module "@tanstack/react-router" {
  interface HistoryState {
    created?: "customers" | "projects";
  }
}
