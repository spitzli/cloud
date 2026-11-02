import { lazy, Suspense, useMemo, useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { databaseCommand, publicBinding } from "@webdock/database-contracts";
import { useI18n } from "@webdock/i18n/react";
import { useApi, useSession } from "./context";
import { Failure, Loading } from "./common";
import { ManagedDatabases } from "./ManagedDatabases";
import * as ui from "./ui";
/** Card list whose first row sits flush against the card border. */
const list = `${ui.card} [&>li:first-child]:border-t-0`;
const Workbench = lazy(async () => {
  // The retained workspace's geometry reader expects the standard browser Buffer shim.
  const { Buffer } = await import("buffer/");
  Object.assign(globalThis, { Buffer });
  const module = await import("@webdock/module-databases/ui/workbench");
  return { default: module.DatabaseWorkbenchWithActions };
});
const directorySchema = z.object({
  data: z.object({
    bindings: z.array(publicBinding),
    managed: z.array(
      z.object({
        id: z.string(),
        projectID: z.string(),
        name: z.string(),
        state: z.string(),
        canOpen: z.boolean(),
      }),
    ),
    operator: z.boolean(),
    projects: z.array(z.object({ id: z.string(), name: z.string() })),
    people: z.array(
      z.object({ id: z.string(), name: z.string(), email: z.string() }),
    ),
  }),
});
const launchSchema = z.object({
  data: z.object({
    code: z.string().regex(/^[A-Za-z0-9_-]{43}$/),
    gatewayOrigin: z.url(),
  }),
});
export function DatabaseDirectory({
  customerID,
}: {
  customerID?: string | undefined;
}) {
  const api = useApi();
  const session = useSession();
  const i18n = useI18n();
  const client = useQueryClient();
  const navigate = useNavigate();
  const tenants = useQuery({
    queryKey: ["tenants"],
    queryFn: () =>
      api.get(
        "/api/console/tenants",
        z.array(z.object({ id: z.string(), name: z.string() })),
      ),
  });
  const directory = useQuery({
    queryKey: ["databases", customerID],
    queryFn: () =>
      api.write(
        "/api/console/databases",
        "POST",
        { action: "list", customerID },
        directorySchema,
      ),
    enabled: Boolean(customerID) && !(session.user.preview && !session.user.acting),
  });
  const [editing, setEditing] = useState<string>();
  const mutation = useMutation({
    mutationFn: (command: unknown) =>
      api.write(
        "/api/console/databases",
        "POST",
        databaseCommand.parse(command),
        z.object({ data: z.unknown() }),
      ),
    onSuccess: () => {
      setEditing(undefined);
      void client.invalidateQueries({ queryKey: ["databases", customerID] });
    },
  });
  if (!api.config.enabledFeatures.includes("databases"))
    return (
      <section>
        <h1 className={ui.pageTitle}>{i18n.t("Databases")}</h1>
        <p className={ui.pageLead}>{i18n.t("Database access is unavailable.")}</p>
      </section>
    );
  if ((session.user.preview && !session.user.acting))
    return (
      <section>
        <h1 className={ui.pageTitle}>{i18n.t("Databases")}</h1>
        <p className={ui.pageLead}>{i18n.t("Customer preview is read-only.")}</p>
      </section>
    );
  if (tenants.isPending) return <Loading>{i18n.t("Loading…")}</Loading>;
  if (tenants.error)
    return (
      <Failure error={tenants.error} retry={() => void tenants.refetch()} />
    );
  const data = directory.data?.data;
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>{i18n.t("Databases")}</h1>
          <p className={ui.pageLead}>
            {i18n.t("Open a database with your assigned access.")}
          </p>
        </div>
        {data?.operator ? (
          <button
            className={ui.button("primary")}
            onClick={() => {
              setEditing("create");
            }}
          >
            {i18n.t("Add database")}
          </button>
        ) : null}
      </div>
      <label className={ui.cx(`${ui.field} mb-6 max-w-[420px]`)}>
        {i18n.t("Workspace")}
        <select className={ui.input}
          aria-label={i18n.t("Workspace")}
          value={customerID ?? ""}
          onChange={(event) =>
            void navigate({
              to: "/databases",
              search: { customer: event.target.value || undefined },
            })
          }
        >
          <option value="">{i18n.t("Choose a workspace.")}</option>
          {tenants.data.map((tenant) => (
            <option key={tenant.id} value={tenant.id}>
              {tenant.name}
            </option>
          ))}
        </select>
      </label>
      {!customerID ? (
        <p className="text-muted">{i18n.t("Choose a workspace.")}</p>
      ) : directory.isPending ? (
        <Loading>{i18n.t("Loading…")}</Loading>
      ) : directory.error ? (
        <Failure
          error={directory.error}
          retry={() => void directory.refetch()}
        />
      ) : null}
      {mutation.error ? (
        <p className="mt-4 text-sm text-danger" role="alert">
          {i18n.error(mutation.error.message)}
        </p>
      ) : null}
      {data && customerID ? (
        <>
          <ManagedDatabases
            key={customerID}
            customerID={customerID}
            projects={data.projects}
            operator={data.operator}
          />
          {data.managed.some((database) => database.canOpen) ? (
            <section className="mt-8 grid gap-3">
              <h2 className={ui.sectionTitle}>{i18n.t("Database workspace access")}</h2>
              {data.managed.length ? (
                <ul className={list}>
                  {data.managed
                    .filter((database) => database.canOpen)
                    .map((database) => (
                      <li key={database.id} className={ui.cx(ui.cardRow, ui.clickableRow)}>
                        <div className="grid min-w-0 gap-1">
                          <strong className="font-medium">{database.name}</strong>
                          <span className={ui.cx(`${ui.badge(ui.statusTone(database.state))} justify-self-start`)}>{database.state}</span>
                        </div>
                        {database.canOpen ? (
                          <Link
                            className={ui.cx(ui.button(), ui.rowLink)}
                            to="/databases/$id"
                            params={{ id: database.id }}
                            search={{ managed: true, customer: customerID }}
                          >
                            {i18n.t("Open database")}
                          </Link>
                        ) : null}
                      </li>
                    ))}
                </ul>
              ) : (
                <p className="text-muted">{i18n.t("No databases available.")}</p>
              )}
            </section>
          ) : null}
          <section className="mt-8 grid gap-3">
            <h2 className={ui.sectionTitle}>{i18n.t("Database connections")}</h2>
            {data.bindings.length ? (
              <ul className={list}>
                {data.bindings.map((binding) => (
                  <li key={binding.id} className={ui.cx(ui.cardRow, binding.profile && ui.clickableRow)}>
                    <div className="grid min-w-0 gap-0.5">
                      <strong className="font-medium">{binding.name}</strong>
                      <p className="text-[13px] text-muted">
                        {binding.engine} · {binding.environment}
                        {binding.profile ? ` · ${binding.profile}` : ""}
                      </p>
                    </div>
                    <div className="flex flex-wrap items-center gap-2">
                      {binding.profile ? (
                        <Link
                          className={ui.cx(ui.button(), ui.rowLink)}
                          to="/databases/$id"
                          params={{ id: binding.id }}
                          search={{ customer: customerID, managed: false }}
                        >
                          {i18n.t("Open database")}
                        </Link>
                      ) : null}
                      {data.operator ? (
                        <button
                          className={ui.cx(ui.button("quiet"), "relative z-10")}
                          onClick={() => {
                            setEditing(binding.id);
                          }}
                        >
                          {i18n.t("Manage access")}
                        </button>
                      ) : null}
                    </div>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-muted">{i18n.t("No databases available.")}</p>
            )}
          </section>
          {data.operator && editing ? (
            <section className={ui.cx(`${ui.card} mt-8`)}>
              <form
                key={editing}
                className={ui.cx(`${ui.cardBody} sm:grid-cols-2`)}
                onSubmit={(event) => {
                  event.preventDefault();
                  const f = new FormData(event.currentTarget);
                  const command =
                    editing === "create"
                      ? {
                          action: "create",
                          projectID: f.get("project"),
                          name: f.get("name"),
                          engine: f.get("engine"),
                          environment: f.get("environment"),
                        }
                      : {
                          action: "grant",
                          bindingID: editing,
                          subject: f.get("subject"),
                          profile: f.get("profile"),
                          runtimeOrigin: f.get("runtime"),
                          connectionID: f.get("connection"),
                          proxySecret: f.get("secret"),
                          isolationVerified: f.get("isolation") === "on",
                          databaseRoleVerified: f.get("role") === "on",
                        };
                  mutation.mutate(command);
                }}
              >
                <h2 className={ui.cx(`${ui.sectionTitle} sm:col-span-2`)}>
                  {i18n.t(
                    editing === "create" ? "Add database" : "Manage access",
                  )}
                </h2>
                {editing === "create" ? (
                  <>
                    <label className={ui.field}>
                      {i18n.t("Project")}
                      <select className={ui.input} name="project" required>
                        {data.projects.map((project) => (
                          <option key={project.id} value={project.id}>
                            {project.name}
                          </option>
                        ))}
                      </select>
                    </label>
                    <label className={ui.field}>
                      {i18n.t("Name")}
                      <input className={ui.input} name="name" required maxLength={120} />
                    </label>
                    <label className={ui.field}>
                      {i18n.t("Database engine")}
                      <select className={ui.input} name="engine">
                        <option value="postgresql">PostgreSQL</option>
                        <option value="sqlite">SQLite</option>
                      </select>
                    </label>
                    <label className={ui.field}>
                      {i18n.t("Environment")}
                      <select className={ui.input} name="environment">
                        <option value="production">Production</option>
                        <option value="staging">Staging</option>
                        <option value="development">Development</option>
                      </select>
                    </label>
                  </>
                ) : (
                  <>
                    <label className={ui.field}>
                      {i18n.t("User")}
                      <select className={ui.input} name="subject" required>
                        {data.people.map((person) => (
                          <option key={person.id} value={person.id}>
                            {person.name} · {person.email}
                          </option>
                        ))}
                      </select>
                    </label>
                    <label className={ui.field}>
                      {i18n.t("Access")}
                      <select className={ui.input} name="profile">
                        <option value="read">{i18n.t("Read only")}</option>
                        <option value="write">
                          {i18n.t("Read and write")}
                        </option>
                        <option value="schema">
                          {i18n.t("Schema administration")}
                        </option>
                      </select>
                    </label>
                    <label className={ui.field}>
                      {i18n.t("Runtime origin")}
                      <input className={ui.input}
                        name="runtime"
                        type="url"
                        required
                        maxLength={300}
                        placeholder="https://database.example.com"
                      />
                    </label>
                    <label className={ui.field}>
                      {i18n.t("Connection ID")}
                      <input className={ui.input} name="connection" required maxLength={128} />
                    </label>
                    <label className={ui.field}>
                      {i18n.t("Proxy secret")}
                      <input className={ui.input}
                        name="secret"
                        type="password"
                        autoComplete="new-password"
                        required
                        minLength={32}
                        maxLength={512}
                      />
                    </label>
                    <label className="flex items-start gap-2.5 text-[13px] sm:col-span-2">
                      <input className="mt-0.5 size-4 shrink-0 accent-accent" name="isolation" type="checkbox" required />
                      {i18n.t(
                        "This runtime is isolated for this database user.",
                      )}
                    </label>
                    <label className="flex items-start gap-2.5 text-[13px] sm:col-span-2">
                      <input className="mt-0.5 size-4 shrink-0 accent-accent" name="role" type="checkbox" required />
                      {i18n.t("The database role matches the selected access.")}
                    </label>
                  </>
                )}
                <div className="flex flex-wrap items-center gap-2 sm:col-span-2">
                  <button
                    className={ui.button("primary")}
                    disabled={mutation.isPending}
                  >
                    {i18n.t("Save")}
                  </button>
                  <button
                    type="button"
                    className={ui.button()}
                    onClick={() => {
                      setEditing(undefined);
                    }}
                  >
                    {i18n.t("Cancel")}
                  </button>
                  {editing !== "create" ? (
                    <button
                      type="button"
                      className={ui.button()}
                      disabled={mutation.isPending}
                      onClick={(event) => {
                        const form = event.currentTarget.closest("form");
                        if (!form) return;
                        const subject = new FormData(form).get("subject");
                        mutation.mutate({
                          action: "revoke",
                          bindingID: editing,
                          subject,
                        });
                      }}
                    >
                      {i18n.t("Revoke access")}
                    </button>
                  ) : null}
                </div>
              </form>
              {editing !== "create" ? (
                <div className={ui.cx(`${ui.cardFooter} justify-end`)}>
                <button
                  className={ui.button("danger")}
                  disabled={mutation.isPending}
                  onClick={() => {
                    if (
                      window.confirm(
                        i18n.t(
                          "Disable this database connection for all users?",
                        ),
                      )
                    )
                      mutation.mutate({
                        action: "disable",
                        bindingID: editing,
                      });
                  }}
                >
                  {i18n.t("Disable connection")}
                </button>
                </div>
              ) : null}
            </section>
          ) : null}
        </>
      ) : null}
    </section>
  );
}
export function DatabaseViewer({
  id,
  managed,
  customerID,
}: {
  id: string;
  managed: boolean;
  customerID?: string | undefined;
}) {
  const api = useApi();
  const session = useSession();
  const i18n = useI18n();
  const actions = useMemo(
    () => ({
      launchDatabase: async (bindingID: string) => ({
        launch: (
          await api.write(
            "/api/console/databases",
            "POST",
            { action: "open", bindingID },
            launchSchema,
          )
        ).data,
      }),
      launchManagedDatabase: async (databaseID: string) => ({
        launch: (
          await api.write(
            "/api/console/databases",
            "POST",
            { action: "openManaged", databaseID },
            launchSchema,
          )
        ).data,
      }),
    }),
    [api],
  );
  if ((session.user.preview && !session.user.acting) || !api.config.enabledFeatures.includes("databases"))
    return <p className="text-muted">{i18n.t("Database access is unavailable.")}</p>;
  return (
    // database-view remains as the hook for the module's fixed-height workbench rules.
    <section className="database-view">
      <div className={ui.cx(`${ui.pageHeading} mb-6`)}>
        <h1 className={ui.pageTitle}>{i18n.t("Database workspace")}</h1>
        <Link
          className={ui.button()}
          to="/databases"
          search={{ customer: customerID }}
        >
          {i18n.t("Close")}
        </Link>
      </div>
      <Suspense
        fallback={<Loading>{i18n.t("Connecting to database…")}</Loading>}
      >
        <Workbench
          bindingID={id}
          userID={session.user.id}
          managed={managed}
          actions={actions}
        />
      </Suspense>
    </section>
  );
}
