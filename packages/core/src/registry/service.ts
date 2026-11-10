import { whereQuery } from "./query";
import type { RegistryDependencies } from "./ports";
import { RegistryError } from "./errors";
export { RegistryError } from "./errors";
export type { RegistryIdentity } from "./ports";
import type { RegistryIdentity } from "./ports";
import {
  and,
  eq,
  sql,
  asc,
  desc,
  ilike,
  count,
  type SQL,
  getTableColumns,
} from "drizzle-orm";
import * as schema from "@spitzli/platform-db/schema/registry";
import { customerInput, projectInput, recordID, listInput } from "./contracts";
const querySources = {
  customers: ["webdock_admin.customers"],
  projects: ["webdock_admin.projects", "webdock_admin.customers"],
  "audit-events": ["webdock_admin.audit_events", "webdock_admin.users"],
  users: ["webdock_admin.users"],
};
export function createRegistryService(dependencies: RegistryDependencies) {
  const {
    database: db,
    pool,
    withSessionScope,
    close,
    readCachedView = async (_options, _version, load) => load(),
  } = dependencies;
  const extensions = dependencies.collections ?? {};
  const tables: Record<
    string,
    import("./ports").RegistryCollectionExtension["table"]
  > = {
    customers: schema.customers,
    projects: schema.projects,
    "audit-events": schema.auditEvents,
    users: schema.users,
  };
  const sources: Record<string, string[]> = { ...querySources };
  for (const [name, extension] of Object.entries(extensions)) {
    if (Object.hasOwn(tables, name))
      throw new RegistryError("Duplicate registry collection.");
    tables[name] = extension.table;
    sources[name] = extension.sources;
  }
  function tableFor(name: string) {
    if (!Object.hasOwn(tables, name))
      throw new RegistryError("Registry collection unavailable.", 404);
    return tables[name];
  }
  type Tx = Parameters<Parameters<typeof db.transaction>[0]>[0];
  const execute = <T>(
    actor: RegistryIdentity,
    run: (tx: Tx, localID: string) => Promise<T>,
  ) =>
    withSessionScope(actor, () =>
      db.transaction(async (tx) => {
        if (
          !(
            await tx.execute<{ allowed: boolean }>(
              sql`SELECT webdock_rls.operator() AS allowed`,
            )
          ).rows[0].allowed
        )
          throw new RegistryError("Current operator access is required.", 403);
        const native = await tx
          .select({ id: schema.users.id })
          .from(schema.users)
          .where(
            and(
              eq(schema.users.authSubject, actor.subject),
              eq(schema.users.id, sql`webdock_rls.portal_user()->>'id'`),
            ),
          )
          .limit(2)
          .for("share");
        if (native.length !== 1)
          throw new RegistryError("A mapped operator is required.", 403);
        const result = await run(tx, native[0].id);
        if (
          !(
            await tx.execute<{ allowed: boolean }>(
              sql`SELECT webdock_rls.operator() AS allowed`,
            )
          ).rows[0].allowed
        )
          throw new RegistryError("Operator access was revoked.", 403);
        return result;
      }),
    ).catch((error: unknown) => {
      const failure = error as { code?: string; cause?: { code?: string } };
      if (failure?.code === "55000" || failure?.cause?.code === "55000")
        throw new RegistryError(
          "Project deletion is in progress. Wait for its result.",
          409,
        );
      throw error;
    });
  const id = async (tx: Tx) =>
    (
      await tx.execute<{ id: string }>(
        sql`SELECT webdock_admin.next_snowflake() AS id`,
      )
    ).rows[0].id;
  async function audit(
    tx: Tx,
    actor: string,
    collection: string,
    before: Record<string, unknown> | undefined,
    after: Record<string, unknown>,
  ) {
    const changed = Object.keys(after).filter(
      (key) =>
        !["id", "createdAt", "updatedAt"].includes(key) &&
        JSON.stringify(before?.[key]) !== JSON.stringify(after[key]),
    );
    await tx.insert(schema.auditEvents).values({
      id: await id(tx),
      actor,
      action: before ? "update" : "create",
      targetCollection: collection,
      targetID: String(after.id),
      summary: `${before ? "Updated" : "Created"} ${String(after.name || after.label || collection)}`,
      changedFields: changed.join(", "),
    });
  }
  async function writeCustomer(
    actor: RegistryIdentity,
    key: string | null,
    input: unknown,
  ) {
    if (key !== null) recordID.parse(key);
    const data = customerInput.parse(input);
    return execute(actor, async (tx, localID) => {
      const old = key
        ? (
            await tx
              .select()
              .from(schema.customers)
              .where(eq(schema.customers.id, recordID.parse(key)))
              .for("update")
          )[0]
        : undefined;
      if (key && !old) throw new RegistryError("Customer unavailable.", 404);
      const rows = old
        ? await tx
            .update(schema.customers)
            .set({ ...data, updatedAt: new Date() })
            .where(eq(schema.customers.id, old.id))
            .returning()
        : await tx
            .insert(schema.customers)
            .values({
              ...data,
              id: await id(tx),
              customerType: data.customerType ?? "company",
              status: "active",
            })
            .returning();
      await audit(tx, localID, "customers", old, rows[0]);
      return rows[0];
    });
  }
  async function writeProject(
    actor: RegistryIdentity,
    key: string | null,
    input: unknown,
  ) {
    if (key !== null) recordID.parse(key);
    const data = projectInput.parse(input);
    return execute(actor, async (tx, localID) => {
      const old = key
        ? (
            await tx
              .select()
              .from(schema.projects)
              .where(eq(schema.projects.id, recordID.parse(key)))
              .for("update")
          )[0]
        : undefined;
      if (key && !old) throw new RegistryError("Project unavailable.", 404);
      if (old && old.customer !== data.customer)
        throw new RegistryError("Project ownership cannot be reassigned here.");
      const parent = (
        await tx
          .select()
          .from(schema.customers)
          .where(eq(schema.customers.id, data.customer))
          .for("share")
      )[0];
      if (!parent || (parent.status !== "active" && old?.status !== "archived"))
        throw new RegistryError("Choose an active customer.");
      const rows = old
        ? await tx
            .update(schema.projects)
            .set({ ...data, updatedAt: new Date() })
            .where(eq(schema.projects.id, old.id))
            .returning()
        : await tx
            .insert(schema.projects)
            .values({ ...data, id: await id(tx), status: "active" })
            .returning();
      await audit(tx, localID, "projects", old, rows[0]);
      return rows[0];
    });
  }
  async function writeCollection(
    actor: RegistryIdentity,
    collection: string,
    key: string | null,
    input: unknown,
    options?: unknown,
  ) {
    const extension = Object.hasOwn(extensions, collection)
      ? extensions[collection]
      : undefined;
    if (!extension)
      throw new RegistryError("Registry collection unavailable.", 404);
    const write = extension.prepareWrite(key, input, options);
    return execute(actor, (tx, localID) => write({ tx, localID, id, audit }));
  }

  async function archive(
    actor: RegistryIdentity,
    collection: "customers" | "projects",
    key: string,
    archived: boolean,
  ) {
    recordID.parse(key);
    return execute(actor, async (tx, localID) => {
      const table =
        collection === "customers" ? schema.customers : schema.projects;
      const old = (
        await tx.select().from(table).where(eq(table.id, key)).for("update")
      )[0];
      if (!old) throw new RegistryError("Record unavailable.", 404);
      if (archived) {
        if (
          collection === "customers" &&
          (
            await tx
              .select({ id: schema.projects.id })
              .from(schema.projects)
              .where(
                and(
                  eq(schema.projects.customer, key),
                  eq(schema.projects.status, "active"),
                ),
              )
              .limit(1)
          ).length
        )
          throw new RegistryError(
            "Archive this customer’s active projects first.",
          );
        if (collection === "projects")
          await dependencies.beforeArchive?.(tx, collection, key);
      } else if (collection === "projects") {
        const project = old as typeof schema.projects.$inferSelect;
        const parent = (
          await tx
            .select()
            .from(schema.customers)
            .where(eq(schema.customers.id, project.customer))
            .for("share")
        )[0];
        if (!parent || parent.status !== "active")
          throw new RegistryError("Choose an active customer.");
      }
      const row = (
        await tx
          .update(table)
          .set({
            status: archived ? "archived" : "active",
            updatedAt: new Date(),
          })
          .where(eq(table.id, key))
          .returning()
      )[0];
      await audit(tx, localID, collection, old, row);
      return row;
    });
  }
  async function listRecords(actor: RegistryIdentity, input: unknown) {
    const name =
      typeof input === "object" && input !== null && "collection" in input
        ? String(input.collection)
        : "";
    if (name === "users") throw new RegistryError("Invalid collection.");
    tableFor(name);
    const extraFilters = Object.fromEntries(
      Object.keys(extensions[name]?.filters ?? {}).map((key) => [
        key,
        recordID.optional(),
      ]),
    );
    const query = listInput
      .extend(extraFilters)
      .parse(input) as import("zod").infer<typeof listInput> &
      Record<string, unknown>;
    return execute(actor, async (tx) => {
      const table = tableFor(query.collection);
      const columns = getTableColumns(table) as Record<
        string,
        import("drizzle-orm/pg-core").AnyPgColumn
      >;
      const extension = extensions[query.collection];
      const filters: SQL[] = [];
      const filterFields: Record<string, string> =
        query.collection === "projects"
          ? { customer: "customer" }
          : query.collection === "audit-events"
            ? { targetID: "targetID", targetCollection: "targetCollection" }
            : (extension?.filters ?? {});
      for (const key of new Set([
        "customer",
        "targetID",
        "targetCollection",
        ...Object.keys(filterFields),
      ])) {
        if (query[key] !== undefined) {
          const field = filterFields[key];
          if (!field || !columns[field])
            throw new RegistryError("Invalid collection filter.");
          filters.push(eq(columns[field], query[key] as string));
        }
      }
      if (query.status) {
        const allowed =
          extension?.statuses ??
          (query.collection === "audit-events" ? [] : ["active", "archived"]);
        if (!allowed.includes(query.status) || !columns.status)
          throw new RegistryError("Invalid status filter.");
        filters.push(eq(columns.status, query.status));
      }
      if (query.search) {
        const field =
          extension?.searchField ??
          (query.collection === "audit-events" ? "summary" : "name");
        filters.push(
          ilike(
            columns[field],
            "%" + query.search.replace(/[\\%_]/g, "\\$&") + "%",
          ),
        );
      }
      const field = query.sort.replace(/^-/, "");
      const order = [
        "createdAt",
        "updatedAt",
        extension?.searchField ?? "name",
      ].includes(field)
        ? columns[field]
        : undefined;
      if (!order) throw new RegistryError("Invalid sort.");
      const totalDocs = (
        await tx
          .select({ n: count() })
          .from(table)
          .where(and(...filters))
      )[0].n;
      const docs = await tx
        .select()
        .from(table)
        .where(and(...filters))
        .orderBy(
          query.sort.startsWith("-") ? desc(order) : asc(order),
          asc(table.id),
        )
        .limit(query.limit)
        .offset((query.page - 1) * query.limit);
      const totalPages = Math.max(1, Math.ceil(totalDocs / query.limit));
      return {
        docs,
        page: query.page,
        totalPages,
        totalDocs,
        hasNextPage: query.page < totalPages,
        hasPrevPage: query.page > 1,
      };
    });
  }
  async function getRecord(
    actor: RegistryIdentity,
    collection: unknown,
    key: string,
  ) {
    if (typeof collection !== "string")
      throw new RegistryError("Invalid collection.");
    const name = collection;
    if (name === "users") throw new RegistryError("Invalid collection.");
    recordID.parse(key);
    return execute(actor, async (tx) => {
      const table = tableFor(name);
      const row = (await tx.select().from(table).where(eq(table.id, key)))[0];
      if (!row) throw new RegistryError("Record unavailable.", 404);
      return row;
    });
  }
  type Collection = string;
  async function operator(actor: RegistryIdentity) {
    return execute(actor, async (tx, localID) => {
      const row = (
        await tx
          .select({
            id: schema.users.id,
            name: schema.users.name,
            email: schema.users.email,
            authSubject: schema.users.authSubject,
            createdAt: schema.users.createdAt,
            updatedAt: schema.users.updatedAt,
          })
          .from(schema.users)
          .where(eq(schema.users.id, localID))
      )[0];
      return JSON.parse(JSON.stringify({ ...row, role: "operator", collection: "users" }));
    });
  }
  async function query(
    actor: RegistryIdentity,
    mode: string,
    raw: Record<string, unknown>,
  ) {
    if (
      !["find", "findByID", "count"].includes(mode) ||
      !Object.hasOwn(tables, String(raw.collection))
    )
      throw new RegistryError("Invalid registry operation.");
    const collection = raw.collection as Collection;
    for (const key of Object.keys(raw))
      if (
        ![
          "collection",
          "where",
          "id",
          "page",
          "limit",
          "sort",
          "depth",
          "pagination",
          "select",
          "disableErrors",
        ].includes(key)
      )
        throw new RegistryError("Invalid query option.");
    const page = raw.page === undefined ? 1 : Number(raw.page),
      limit = raw.limit === undefined ? 20 : Number(raw.limit),
      depth = raw.depth === undefined ? 1 : Number(raw.depth);
    if (
      !Number.isSafeInteger(page) ||
      page < 1 ||
      page > 10000 ||
      !Number.isSafeInteger(limit) ||
      limit < 1 ||
      limit > 1000 ||
      !Number.isInteger(depth) ||
      depth < 0 ||
      depth > 2
    )
      throw new RegistryError("Invalid pagination.");
    return execute(actor, async (tx) =>
      readCachedView(
        {
          kind: "registry",
          scope: actor,
          query: JSON.parse(
            JSON.stringify({ ...raw, mode, page, limit, depth }),
          ),
          ttlSeconds: 60,
        },
        async () =>
          (
            await tx.execute<{ version: string }>(
              sql`SELECT webdock_cache.version(${sql.param(sources[collection])}::text[]) AS version`,
            )
          ).rows[0].version,
        async () => {
          const table = tables[collection];
          const columns = { ...getTableColumns(table) };
          if (collection === "users")
            for (const key of Object.keys(columns))
              if (
                ![
                  "id",
                  "name",
                  "email",
                  "role",
                  "authSubject",
                  "createdAt",
                  "updatedAt",
                ].includes(key)
              )
                delete columns[key as keyof typeof columns];
          const filter = and(
            whereQuery(raw.where, columns),
            mode === "findByID"
              ? eq(table.id, recordID.parse(raw.id))
              : undefined,
          );
          if (mode === "count")
            return {
              totalDocs: (
                await tx.select({ n: count() }).from(table).where(filter)
              )[0].n,
            };
          const sorts = Array.isArray(raw.sort)
            ? raw.sort
            : [raw.sort || "-updatedAt"];
          if (sorts.length > 3) throw new RegistryError("Invalid sort.");
          const order = sorts.map((sort) => {
            if (typeof sort !== "string")
              throw new RegistryError("Invalid sort.");
            const field = sort.replace(/^-/, "");
            const column = Object.hasOwn(columns, field)
              ? columns[field as keyof typeof columns]
              : undefined;
            if (!column) throw new RegistryError("Invalid sort.");
            return sort.startsWith("-") ? desc(column) : asc(column);
          });
          const allColumns = { ...columns };
          if (raw.select !== undefined) {
            if (
              !raw.select ||
              typeof raw.select !== "object" ||
              Array.isArray(raw.select)
            )
              throw new RegistryError("Invalid selection.");
            for (const [field, selected] of Object.entries(raw.select))
              if (
                !Object.hasOwn(columns, field) ||
                typeof selected !== "boolean"
              )
                throw new RegistryError("Invalid selection.");
            const selection = raw.select as Record<string, boolean>,
              include = Object.values(selection).some(Boolean);
            for (const field of Object.keys(allColumns))
              if (
                field !== "id" &&
                (include
                  ? selection[field] !== true
                  : selection[field] === false)
              )
                delete allColumns[field as keyof typeof allColumns];
          }
          const docs = await tx
            .select(allColumns)
            .from(table)
            .where(filter)
            .orderBy(...order, asc(table.id))
            .limit(mode === "findByID" ? 1 : limit)
            .offset(mode === "findByID" ? 0 : (page - 1) * limit);
          async function populate(
            row: Record<string, unknown>,
            kind: Collection,
            level: number,
          ): Promise<Record<string, unknown>> {
            if (!level) return row;
            const relation =
              kind === "projects"
                ? ["customer", "customers"]
                : kind === "audit-events"
                  ? ["actor", "users"]
                  : (extensions[kind]?.relation ?? null);
            if (relation && typeof row[relation[0]] === "string") {
              const parentTable = tables[relation[1] as Collection];
              const safe = { ...getTableColumns(parentTable) };
              if (relation[1] === "users")
                for (const key of Object.keys(safe))
                  if (
                    ![
                      "id",
                      "name",
                      "email",
                      "role",
                      "authSubject",
                      "createdAt",
                      "updatedAt",
                    ].includes(key)
                  )
                    delete safe[key as keyof typeof safe];
              const parent = (
                await tx
                  .select(safe)
                  .from(parentTable)
                  .where(eq(parentTable.id, String(row[relation[0]])))
              )[0];
              if (parent)
                row[relation[0]] = await populate(
                  parent,
                  relation[1] as Collection,
                  level - 1,
                );
            }
            return row;
          }
          const result = [];
          for (const doc of docs)
            result.push(await populate(doc, collection, depth));
          const serial = JSON.parse(JSON.stringify(result));
          if (mode === "findByID") {
            if (!serial[0] && !raw.disableErrors)
              throw new RegistryError("Record unavailable.", 404);
            return serial[0] || null;
          }
          const totalDocs = (
              await tx.select({ n: count() }).from(table).where(filter)
            )[0].n,
            totalPages = Math.max(1, Math.ceil(totalDocs / limit));
          return {
            docs: serial,
            totalDocs,
            totalPages,
            page,
            limit,
            hasNextPage: page < totalPages,
            hasPrevPage: page > 1,
            nextPage: page < totalPages ? page + 1 : null,
            prevPage: page > 1 ? page - 1 : null,
          };
        },
      ),
    );
  }
  async function patchRepository(
    actor: RegistryIdentity,
    key: string,
    url: string,
  ) {
    projectInput.shape.repositoryURL.parse(url);
    recordID.parse(key);
    return execute(actor, async (tx, localID) => {
      const old = (
        await tx
          .select()
          .from(schema.projects)
          .where(eq(schema.projects.id, key))
          .for("update")
      )[0];
      if (!old) throw new RegistryError("Project unavailable.", 404);
      if (old.status !== "active")
        throw new RegistryError(
          "Restore this project before linking a repository.",
          409,
        );
      const row = (
        await tx
          .update(schema.projects)
          .set({ repositoryURL: url, updatedAt: new Date() })
          .where(eq(schema.projects.id, key))
          .returning()
      )[0];
      await audit(tx, localID, "projects", old, row);
      return row;
    });
  }
  return {
    writeCustomer,
    writeProject,
    writeCollection,
    archive,
    listRecords,
    getRecord,
    query,
    operator,
    patchRepository,
    raw: pool,
    close,
  };
}
