import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { useI18n } from "@webdock/i18n/react";
import { DatabaseCredentialPanelWithActions } from "@webdock/module-databases/ui/database-details";
import { useApi, useSession } from "./context";
import { sessionCacheScope } from "./session-cache";
import { Failure, Loading } from "./common";
import * as ui from "./ui";
import { DatabaseBackups } from "./DatabaseBackups";
import { useBackupProjectSelection, useBackupRequestMemory, useRetireBackupResource } from "./database-backup-intents";
import {
  acceptReviewedOffer,
  catalogSchema,
  databaseCommandSchema,
  databaseViewSchema,
  offerSchema,
  planEntry,
  planFromForm,
  type Offer,
  type PlanEntry,
} from "./database-management";
const response = <T,>(schema: z.ZodType<T>) => z.object({ data: schema });
const permission = z.object({
  canWrite: z.boolean(),
  operator: z.boolean().optional(),
});
const receipt = z.object({
  orderID: z.string(),
  databaseID: z.string(),
  operationID: z.string(),
});
const credentials = z.object({
  databaseID: z.string(),
  databaseName: z.string(),
  host: z.string(),
  directPort: z.number(),
  pooledPort: z.number(),
  credentialRevision: z.number(),
  application: z.object({ username: z.string(), password: z.string() }),
  migration: z.object({ username: z.string(), password: z.string() }),
  sslmode: z.string(),
  caCertificate: z.string(),
});
const previewSchema = z.object({
  databaseID: z.string(),
  name: z.string(),
  revision: z.number(),
  planHash: z.string(),
  retainedUntil: z.string().nullable().optional(),
  expiresAt: z.string().optional(),
});
const key = () => crypto.randomUUID();
/** Card list whose first row sits flush against the card border. */
const list = `${ui.card} [&>li:first-child]:border-t-0`;
const summary =
  "flex cursor-pointer list-none items-center justify-between gap-3 px-6 py-4 font-semibold hover:bg-subtle max-sm:px-[18px] [&::-webkit-details-marker]:hidden " +
  "after:text-muted after:content-['+'] group-open/details:after:content-['−']";
function useManagedCommand() {
  const api = useApi();
  return useCallback(
    async <T,>(input: unknown, schema: z.ZodType<T>) =>
      (
        await api.write(
          "/api/console/databases",
          "POST",
          databaseCommandSchema.parse(input),
          response(schema),
        )
      ).data,
    [api],
  );
}
interface ManagedProps {
  customerID: string;
  projects: { id: string; name: string }[];
  operator: boolean;
}
export function ManagedDatabases(props: ManagedProps) {
  const session = useSession(), api = useApi();
  // Identity owns unresolved effects; changing privileges only retires private views/drafts.
  const identity = JSON.stringify([api.config.apiOrigin, session.user.id, session.session.userId, session.session.id, session.user.customerScope ?? null, session.user.acting ?? false, props.customerID]);
  const scope = JSON.stringify([api.config.apiOrigin, sessionCacheScope(session), props.customerID, props.projects.map(p => p.id)]);
  return <ScopedManagedDatabases key={identity} {...props} scope={scope} />;
}
function ScopedManagedDatabases({
  customerID,
  projects,
  operator,
  scope,
}: ManagedProps & { scope: string }) {
  const { t } = useI18n();
  const newOrdersEnabled = useApi().config.databaseOrderingEnabled !== false;
  const call = useManagedCommand();
  const client = useQueryClient();
  const retireBackupResource = useRetireBackupResource();
  const selectBackupProject = useBackupProjectSelection();
  const [projectID, setProject] = useState(projects[0]?.id ?? "");
  const [selected, setSelected] = useState<string>();
  const [detailOpen, setDetailOpen] = useState(false);
  const [ordering, setOrdering] = useState(false);
  const permissionEpochRef = useRef(0), permissionScopeRef = useRef(scope);
  useLayoutEffect(() => { permissionScopeRef.current = scope; permissionEpochRef.current++; }, [scope]);
  const [backupReadBlocked, setBackupReadBlocked] = useState<Set<string>>(() => new Set());
  const projectAvailable = projects.some(project => project.id === projectID);
  const firstProject = projects[0]?.id ?? "";
  if (!projectAvailable && projectID !== firstProject) {
    setProject(firstProject);
    setSelected(undefined);
    setDetailOpen(false);
    setOrdering(false);
  }
  const backupReadDenied = useCallback(() => { permissionEpochRef.current++; setBackupReadBlocked(prior => new Set(prior).add(projectID)); }, [projectID]);
  useEffect(() => () => { void client.cancelQueries({ queryKey: ["database-management", scope] }); client.removeQueries({ queryKey: ["database-management", scope] }); }, [client, scope]);
  const permissions = useQuery({
    queryKey: ["database-management", scope, projectID, "permissions"],
    queryFn: async () => {
      try { return { access: await call({ action: "databases.permissions", projectID }, permission), readID: crypto.randomUUID() }; }
      catch (error) { if (permissionScopeRef.current === scope) backupReadDenied(); throw error; }
    },
    enabled: Boolean(projectID) && projectAvailable,
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  const readable = projectAvailable && permissions.isSuccess && !backupReadBlocked.has(projectID);
  const canWrite = readable && permissions.data.access.canWrite;
  const revalidateBackupPermissions = async () => {
    const epoch = ++permissionEpochRef.current, oldRead = permissions.data?.readID;
    const current = await permissions.refetch();
    if (permissionEpochRef.current !== epoch || permissionScopeRef.current !== scope) return false;
    const fresh = current.isSuccess && current.data.readID !== oldRead;
    setBackupReadBlocked(prior => { const next = new Set(prior); if (fresh) next.delete(projectID); else next.add(projectID); return next; });
    return fresh;
  };
  const plans = useQuery({
    queryKey: ["database-management", scope, projectID, "plans"],
    queryFn: () =>
      call({ action: "databases.plans.list", projectID }, catalogSchema),
    enabled: Boolean(projectID) && readable,
  });
  const databases = useQuery({
    queryKey: ["database-management", scope, projectID, "list"],
    queryFn: () =>
      call(
        { action: "databases.list", projectID },
        z.array(databaseViewSchema),
      ),
    enabled: Boolean(projectID) && readable,
    refetchInterval: 15000,
  });
  const refresh = () => {
    void client.invalidateQueries({ queryKey: ["database-management"] });
    void client.invalidateQueries({ queryKey: ["databases", customerID] });
  };
  return (
    // database-* class names remain as hooks: the module-rendered credential panel and the live browser test use them.
    <section className="database-management mt-8 grid gap-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className={ui.sectionTitle}>{t("Managed databases")}</h2>
        {canWrite && newOrdersEnabled ? (
          <button
            className={ui.button(ordering ? "secondary" : "primary")}
            onClick={() => {
              setOrdering(!ordering);
            }}
          >
            {t(ordering ? "Cancel" : "Order database")}
          </button>
        ) : null}
      </div>
      {!newOrdersEnabled ? (
        <p role="status" className={ui.notice}>
          {t("New shared and self-hosted PostgreSQL orders are retired. For a new database, create a project in your own Neon account. Connect your Neon account in Connections. Existing databases remain available below.")}{" "}
          <a href={`/workspaces/${customerID}/connections`}>{t("Open connections")}</a>{" · "}
          <a href="https://console.neon.tech" target="_blank" rel="noopener noreferrer">{t("Open Neon Console")}</a>
        </p>
      ) : null}
      <label className={ui.cx(`${ui.field} max-w-[420px]`)}>
        {t("Project")}
        <select className={ui.input}
          value={projectID}
          onChange={(event) => {
            const nextProject = event.target.value;
            if (nextProject === projectID) return;
            selectBackupProject(customerID, nextProject);
            void client.cancelQueries({ queryKey: ["database-management", scope] });
            client.removeQueries({ queryKey: ["database-management", scope] });
            setBackupReadBlocked(prior => { const next = new Set(prior); next.delete(nextProject); return next; });
            setProject(nextProject);
            setSelected(undefined);
            setDetailOpen(false);
            setOrdering(false);
          }}
        >
          {projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name}
            </option>
          ))}
        </select>
      </label>
      {permissions.error ? <Failure error={permissions.error} retry={() => { void revalidateBackupPermissions(); }} /> : null}
      {plans.error ? (
        <Failure error={plans.error} retry={() => void plans.refetch()} />
      ) : null}
      {plans.data?.unavailable ? (
        <p role="status" className={ui.notice}>
          {t(
            "Some database plans are unavailable because their configuration is invalid.",
          )}
        </p>
      ) : null}
      {ordering && canWrite && newOrdersEnabled && plans.data ? (
        <DatabaseOrder
          key={projectID}
          projectID={projectID}
          plans={plans.data.plans.filter((p) => p.availability !== "disabled")}
          onAccepted={(id) => {
            setSelected(id);
            setDetailOpen(true);
            setOrdering(false);
            refresh();
          }}
        />
      ) : null}
      {databases.isPending && projectID ? (
        <Loading />
      ) : databases.error ? (
        <Failure
          error={databases.error}
          retry={() => void databases.refetch()}
        />
      ) : null}
      {databases.data?.length ? (
        <ul className={list}>
          {databases.data.map((database) => (
            <li key={database.id} className={ui.cx(ui.cardRow, ui.clickableRow)}>
              <div className="grid min-w-0 gap-0.5">
                <strong className="font-medium">{database.name}</strong>
                <span className="flex flex-wrap items-center gap-2 text-[13px] text-muted">
                  PostgreSQL {database.postgresMajor}
                  <span className={ui.badge(ui.statusTone(database.state))}>{t(database.state)}</span>
                </span>
              </div>
              <button
                className={ui.cx(ui.button(), ui.rowLink)}
                onClick={() => {
                  setSelected(database.id);
                  if (selected && selected !== database.id) retireBackupResource();
                  setDetailOpen(true);
                }}
              >
                {t("Manage database")}
              </button>
            </li>
          ))}
        </ul>
      ) : databases.data ? (
        <p className="text-muted">{t("No databases available.")}</p>
      ) : null}
      {selected ? (
        <DatabaseManagement
          key={selected}
          id={selected}
          customerID={customerID}
          open={detailOpen}
          scope={scope}
          projectID={projectID}
          operator={operator && permissions.isSuccess && permissions.data.access.operator === true}
          canWrite={canWrite}
          readable={readable}
          revalidateBackupPermissions={revalidateBackupPermissions}
          backupReadDenied={backupReadDenied}
          plans={plans.data?.plans ?? []}
          close={() => {
            setDetailOpen(false);
          }}
          refresh={refresh}
        />
      ) : null}
      {operator ? (
        <>
          <PlanCatalog plans={plans.data?.plans ?? []} refresh={refresh} />
          <DatabaseCapacity />
        </>
      ) : null}
    </section>
  );
}
function DatabaseOrder({
  projectID,
  plans,
  database,
  onAccepted,
}: {
  projectID: string;
  plans: PlanEntry[];
  database?: { id: string; name: string };
  onAccepted: (id: string) => void;
}) {
  const { t, error, locale } = useI18n();
  const call = useManagedCommand();
  const [offer, setOffer] = useState<Offer>();
  const prepare = useMutation({
    mutationFn: async (form: FormData) =>
      call(
        {
          action: "databases.offers.create",
          projectID,
          planRevisionID: form.get("plan"),
          name: form.get("name"),
          ...(database ? { databaseID: database.id } : {}),
          idempotencyKey: key(),
        },
        offerSchema,
      ),
    onSuccess: setOffer,
  });
  const accept = useMutation({
    mutationFn: async (form: FormData) => {
      if (!offer) throw Error("Review the order first.");
      return call(acceptReviewedOffer(projectID, offer, form), receipt);
    },
    onSuccess: (result) => {
      onAccepted(result.databaseID);
    },
  });
  const money = (cents: number) =>
    new Intl.NumberFormat(locale, {
      style: "currency",
      currency: "EUR",
    }).format(cents / 100);
  return (
    <div className={ui.cx(`database-order ${ui.card} mt-0 pt-0`)}>
      <form
        className={ui.cx(`${ui.cardBody} sm:grid-cols-2`)}
        onChange={() => {
          setOffer(undefined);
        }}
        onSubmit={(event) => {
          event.preventDefault();
          prepare.mutate(new FormData(event.currentTarget));
        }}
      >
        <h3 className={ui.cx(`${ui.sectionTitle} sm:col-span-2`)}>{t(database ? "Upgrade database" : "Order database")}</h3>
        <label className={ui.field}>
          {t("Name")}
          <input className={ui.input}
            name="name"
            required
            maxLength={160}
            defaultValue={database?.name}
            readOnly={Boolean(database)}
          />
        </label>
        <label className={ui.field}>
          {t("Database plan")}
          <select className={ui.input} name="plan" aria-label={t("Database plan")} required>
            {plans.map((plan) => (
              <option key={plan.id} value={plan.id}>
                {plan.plan.name} · {money(plan.plan.monthlyPriceCents)}{" "}
                {t("net / month")}
              </option>
            ))}
          </select>
        </label>
        <div className="flex flex-wrap items-center gap-3 sm:col-span-2">
          <button
            className={ui.button()}
            disabled={prepare.isPending || !plans.length}
          >
            {t("Review paid order")}
          </button>
        </div>
      </form>
      {!plans.length ? (
        <p className="border-t border-outline px-6 py-3.5 text-sm text-muted max-sm:px-[18px]">{t("No database plans are available yet.")}</p>
      ) : null}
      {prepare.error ? (
        <p role="alert" className="border-t border-outline px-6 py-3.5 text-sm text-danger max-sm:px-[18px]">
          {error(prepare.error.message)}
        </p>
      ) : null}
      {offer ? (
        <form
          className="grid gap-3 border-t border-outline bg-subtle px-6 py-5 text-sm max-sm:px-[18px]"
          onSubmit={(event) => {
            event.preventDefault();
            accept.mutate(new FormData(event.currentTarget));
          }}
        >
          <h4 className="font-semibold">{offer.name}</h4>
          <p>
            <strong className="text-base font-semibold">
              {money(offer.snapshot.monthlyPriceCents)} {t("net / month")}
            </strong>
          </p>
          <p className="whitespace-pre-wrap text-muted">{offer.snapshot.terms}</p>
          <p>
            {t("Backups are not configured. No recovery window is included.")}
          </p>
          <p>
            {t(
              "Billing starts after confirmed provisioning. Pausing does not reduce the monthly price.",
            )}
          </p>
          <label className="flex items-start gap-2.5 text-[13px]">
            <input className="mt-0.5 size-4 shrink-0 accent-accent" name="confirmPrice" type="checkbox" value="yes" required />
            {t("I accept this monthly price and the displayed terms.")}
          </label>
          {offer.snapshot.requiresInterruption ? (
            <label className="flex items-start gap-2.5 text-[13px]">
              <input className="mt-0.5 size-4 shrink-0 accent-accent"
                name="confirmInterruption"
                type="checkbox"
                value="yes"
                required
              />
              {t(
                "I agree to interrupt database connections for this upgrade now.",
              )}
            </label>
          ) : null}
          {accept.error ? (
            <p role="alert" className="text-danger">
              {error(accept.error.message)}
            </p>
          ) : null}
          <button className={ui.cx(`${ui.button("primary")} justify-self-start whitespace-normal`)} disabled={accept.isPending}>
            {t(
              accept.isPending
                ? "Ordering…"
                : "Order database with payment obligation",
            )}
          </button>
        </form>
      ) : null}
    </div>
  );
}
function DatabaseManagement({
  id,
  customerID,
  open,
  scope,
  projectID,
  operator,
  canWrite,
  readable,
  revalidateBackupPermissions,
  backupReadDenied,
  plans,
  close,
  refresh,
}: {
  id: string;
  customerID: string;
  open: boolean;
  scope: string;
  projectID: string;
  operator: boolean;
  canWrite: boolean;
  readable: boolean;
  revalidateBackupPermissions: () => Promise<boolean>;
  backupReadDenied: () => void;
  plans: PlanEntry[];
  close: () => void;
  refresh: () => void;
}) {
  const { t, error, locale } = useI18n();
  const call = useManagedCommand();
  const api = useApi();
  const client = useQueryClient();
  const backupMemory = useBackupRequestMemory(customerID, projectID, id);
  const [upgrade, setUpgrade] = useState(false);
  const [preview, setPreview] = useState<{
    kind: "delete" | "purge";
    value: z.infer<typeof previewSchema>;
  }>();
  const database = useQuery({
    queryKey: ["database-management", scope, id, "detail"],
    queryFn: () =>
      call({ action: "databases.get", databaseID: id }, databaseViewSchema),
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    refetchInterval: query => open && !query.state.error ? 10000 : false,
    enabled: readable && open,
  });
  useLayoutEffect(() => { if (database.error) backupReadDenied(); }, [database.error, backupReadDenied]);
  const mutate = useMutation({
    mutationFn: (command: unknown) => call(command, z.unknown()),
    onSuccess: () => {
      setPreview(undefined);
      refresh();
      void client.invalidateQueries({ queryKey: ["database-management", scope, id] });
    },
  });
  const prepare = useMutation({
    mutationFn: async (kind: "delete" | "purge") => ({
      kind,
      value: await call(
        {
          action:
            kind === "delete"
              ? "databases.deletionPreview"
              : "databases.purgePreview",
          databaseID: id,
        },
        previewSchema,
      ),
    }),
    onSuccess: setPreview,
  });
  const reveal = useCallback(
    async (databaseID: string) => {
      try {
        return {
          credentials: (
            await api.write(
              "/api/console/databases",
              "POST",
              { action: "databases.credentials", databaseID },
              response(credentials),
            )
          ).data,
        };
      } catch (e) {
        return {
          error:
            e instanceof Error ? e.message : "Database access is unavailable.",
        };
      }
    },
    [api],
  );
  if (!open) return null;
  if (database.isPending) return <Loading />;
  if (database.error)
    return (
      <Failure error={database.error} retry={() => { void revalidateBackupPermissions().then(valid => { if (valid) void database.refetch(); }); }} />
    );
  const d = database.data;
  const action = (name: string) => {
    mutate.mutate({
      action: `databases.${name}`,
      databaseID: id,
      revision: d.revision,
      idempotencyKey: key(),
    });
  };
  const ready = ["ready", "degraded"].includes(d.state);
  const pending = mutate.isPending || prepare.isPending;
  return (
    <section className={ui.cx(`database-details ${ui.card} mt-0 block pt-0`)}>
      <div className={ui.cardBody}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h3 className={ui.sectionTitle}>{d.name}</h3>
          <p className="mt-1 flex flex-wrap items-center gap-2 text-[13px] text-muted">
            <span className={ui.badge(ui.statusTone(d.state))}>{t(d.state)}</span>
            {t(d.kind === "shared" ? "Shared" : "Dedicated instance")}
          </p>
        </div>
        <button className={ui.button("quiet")} onClick={close}>
          {t("Close")}
        </button>
      </div>
      <dl className="grid gap-4 sm:grid-cols-2 [&_dd]:mt-1 [&_dd]:text-sm [&_dd]:[overflow-wrap:anywhere] [&_dt]:text-xs [&_dt]:text-muted">
        <div>
          <dt>{t("Database")}</dt>
          <dd>
            <code className={ui.mono}>{d.databaseName}</code>
          </dd>
        </div>
        <div>
          <dt>{t("Storage")}</dt>
          <dd>
            {new Intl.NumberFormat(locale, { maximumFractionDigits: 2 }).format(
              (d.volumeBytes ?? d.logicalStorageAllowanceBytes ?? 0) /
                1073741824,
            )}{" "}
            GiB
          </dd>
        </div>
        {d.usage ? (
          <div>
            <dt>{t("Usage")}</dt>
            <dd>
              {new Intl.NumberFormat(locale, {
                maximumFractionDigits: 2,
              }).format(d.usage.logicalBytes / 1048576)}{" "}
              MiB
            </dd>
          </div>
        ) : null}
        {d.endpoint ? (
          <div>
            <dt>{t("Host")}</dt>
            <dd>{d.endpoint.host}</dd>
          </div>
        ) : null}
      </dl>
      <DatabaseBackups memory={backupMemory} database={d} operator={operator} canWrite={canWrite} readable={readable} revalidate={revalidateBackupPermissions} readDenied={backupReadDenied} refresh={refresh} />
      {mutate.error || prepare.error ? (
        <p role="alert" className="text-sm text-danger">
          {error((mutate.error ?? prepare.error)?.message)}
        </p>
      ) : null}
      {canWrite ? (
        <>
          <div className="flex flex-wrap items-center gap-2">
            {ready ? (
              <button
                className={ui.button()}
                disabled={pending}
                onClick={() => {
                  action("pause");
                }}
              >
                {t("Pause")}
              </button>
            ) : null}
            {d.state === "paused" ? (
              <button
                className={ui.button()}
                disabled={pending}
                onClick={() => {
                  action("resume");
                }}
              >
                {t("Resume")}
              </button>
            ) : null}
            {ready && d.kind === "dedicated" ? (
              <button
                className={ui.button()}
                disabled={pending}
                onClick={() => {
                  if (
                    window.confirm(
                      t(
                        "Restart this database and interrupt current connections?",
                      ),
                    )
                  )
                    action("restart");
                }}
              >
                {t("Restart")}
              </button>
            ) : null}
            {d.state === "ready" ? (
              <>
                <button
                  className={ui.button()}
                  disabled={pending}
                  onClick={() => {
                    if (
                      window.confirm(
                        t(
                          "Rotate database credentials? Existing connection passwords will stop working.",
                        ),
                      )
                    )
                      action("rotateCredentials");
                  }}
                >
                  {t("Rotate credentials")}
                </button>
                <button
                  className={ui.button()}
                  onClick={() => {
                    setUpgrade(!upgrade);
                  }}
                >
                  {t(upgrade ? "Cancel" : "Upgrade database")}
                </button>
              </>
            ) : null}
            {!d.cancellationRequestedAt ? (
              <button
                className={ui.button()}
                disabled={pending}
                onClick={() => {
                  if (
                    window.confirm(
                      t("Request cancellation of this database order?"),
                    )
                  )
                    action("requestCancellation");
                }}
              >
                {t("Request cancellation")}
              </button>
            ) : (
              <span className={ui.badge("warning")}>{t("Cancellation requested")}</span>
            )}
            {operator ? (
              <>
                <button
                  className={ui.button("danger")}
                  disabled={pending || ["retained", "purged"].includes(d.state)}
                  onClick={() => {
                    prepare.mutate("delete");
                  }}
                >
                  {t("Delete database")}
                </button>
                {d.state === "retained" ? (
                  <button
                    className={ui.button("danger")}
                    disabled={pending}
                    onClick={() => {
                      prepare.mutate("purge");
                    }}
                  >
                    {t("Permanently delete retained data")}
                  </button>
                ) : null}
                {d.state === "needs-reconciliation" ? (
                  <button
                    className={ui.button()}
                    disabled={pending}
                    onClick={() => {
                      action("reconcile");
                    }}
                  >
                    {t("Reconcile")}
                  </button>
                ) : null}
              </>
            ) : null}
          </div>
          {preview ? (
            <form
              className="grid gap-3 rounded-lg border border-danger/40 bg-danger-soft/40 p-5 text-sm"
              onSubmit={(event) => {
                event.preventDefault();
                const form = new FormData(event.currentTarget);
                mutate.mutate({
                  action: `databases.${preview.kind}`,
                  databaseID: id,
                  revision: preview.value.revision,
                  planHash: preview.value.planHash,
                  confirmName: form.get("confirmName"),
                  idempotencyKey: key(),
                });
              }}
            >
              <h4 className="font-semibold">
                {t(
                  preview.kind === "delete"
                    ? "Delete database"
                    : "Permanently delete retained data",
                )}
              </h4>
              <p>
                {t(
                  preview.kind === "delete"
                    ? "The database will be suspended and its data retained for seven days."
                    : "This permanently deletes the retained database data after its retention period.",
                )}
              </p>
              {preview.value.retainedUntil ? (
                <p>
                  {t("Retained until")}:{" "}
                  {new Date(preview.value.retainedUntil).toLocaleString(locale)}
                </p>
              ) : null}
              <label className={ui.field}>
                {t("Confirm database name")}
                <input className={ui.input}
                  name="confirmName"
                  required
                  autoComplete="off"
                  placeholder={preview.value.name}
                />
              </label>
              <div className="flex flex-wrap items-center gap-2">
                <button className={ui.button("danger")} disabled={pending}>
                  {t("Confirm deletion")}
                </button>
                <button
                  type="button"
                  className={ui.button()}
                  onClick={() => {
                    setPreview(undefined);
                  }}
                >
                  {t("Cancel")}
                </button>
              </div>
            </form>
          ) : null}
          {upgrade && d.state === "ready" ? (
            <DatabaseOrder
              projectID={projectID}
              plans={plans.filter((p) => p.availability !== "disabled")}
              database={d}
              onAccepted={() => {
                setUpgrade(false);
                refresh();
              }}
            />
          ) : null}
          {["ready", "paused"].includes(d.state) ? (
            <DatabaseCredentialPanelWithActions
              key={`${id}:${String(d.revision)}:${d.state}`}
              databaseID={id}
              revealDatabaseCredentials={reveal}
            />
          ) : null}
        </>
      ) : null}
      </div>
    </section>
  );
}
function PlanCatalog({
  plans,
  refresh,
}: {
  plans: PlanEntry[];
  refresh: () => void;
}) {
  const { t, error } = useI18n();
  const call = useManagedCommand();
  const [kind, setKind] = useState("dedicated");
  const mutation = useMutation({
    mutationFn: (command: unknown) => call(command, z.unknown()),
    onSuccess: refresh,
  });
  const create = useMutation({
    mutationFn: (form: FormData) =>
      call(
        {
          action: "databases.plans.create",
          plan: planFromForm(form),
          idempotencyKey: key(),
        },
        planEntry,
      ),
    onSuccess: refresh,
  });
  return (
    <details className={ui.cx(`database-catalog ${ui.card} group/details mt-0 pt-0`)}>
      <summary className={summary}>{t("Database plans")}</summary>
      {mutation.error || create.error ? (
        <p className="border-t border-outline px-6 py-3.5 text-sm text-danger max-sm:px-[18px]" role="alert">
          {error((mutation.error ?? create.error)?.message)}
        </p>
      ) : null}
      <ul className="max-h-80 overflow-auto">
        {plans.map((plan) => (
          <li key={plan.id} className={ui.cardRow}>
            <strong className="font-medium">{plan.plan.name}</strong>
            <label className={ui.cx(`${ui.field} min-w-[150px] max-sm:min-w-[120px]`)}>
              {t("Availability")}
              <select className={ui.input}
                aria-label={`${plan.plan.name} ${t("Availability")}`}
                value={plan.availability}
                disabled={mutation.isPending}
                onChange={(event) => {
                  mutation.mutate({
                    action: "databases.plans.setAvailability",
                    planRevisionID: plan.id,
                    revision: plan.revision,
                    availability: event.target.value,
                    idempotencyKey: key(),
                  });
                }}
              >
                <option value="disabled">{t("Disabled")}</option>
                <option value="pilot">{t("Private pilot")}</option>
                <option value="public">{t("Public")}</option>
              </select>
            </label>
          </li>
        ))}
      </ul>
      <form
        className={ui.cx(`${ui.cardBody} border-t border-outline sm:grid-cols-2`)}
        onSubmit={(event) => {
          event.preventDefault();
          create.mutate(new FormData(event.currentTarget));
        }}
      >
        <h3 className={ui.cx(`${ui.sectionTitle} sm:col-span-2`)}>{t("Create disabled plan")}</h3>
        <label className={ui.field}>
          {t("Name")}
          <input className={ui.input} name="name" required maxLength={160} />
        </label>
        <label className={ui.field}>
          {t("Database kind")}
          <select className={ui.input}
            name="kind"
            value={kind}
            onChange={(event) => {
              setKind(event.target.value);
            }}
          >
            <option value="dedicated">{t("Dedicated instance")}</option>
            <option value="shared">{t("Shared")}</option>
          </select>
        </label>
        <label className={ui.field}>
          {t("Monthly net price (EUR)")}
          <input className={ui.input}
            name="monthlyPrice"
            inputMode="decimal"
            required
            placeholder="12.50"
          />
        </label>
        <label className={ui.field}>
          {t(
            kind === "shared"
              ? "Logical storage allowance (GiB)"
              : "Filesystem size (GiB)",
          )}
          <input className={ui.input}
            name="volume"
            type="number"
            min="1"
            step="1"
            required
            defaultValue="10"
          />
        </label>
        {kind === "dedicated" ? (
          <>
            <label className={ui.field}>
              {t("CPU (millicores)")}
              <input className={ui.input}
                name="cpu"
                type="number"
                min="100"
                step="100"
                required
                defaultValue="500"
              />
            </label>
            <label className={ui.field}>
              {t("Memory (GiB)")}
              <input className={ui.input}
                name="memory"
                type="number"
                min="0.125"
                step="0.125"
                required
                defaultValue="1"
              />
            </label>
          </>
        ) : null}
        <label className={ui.cx(`${ui.field} sm:col-span-2`)}>
          {t("Contract terms")}
          <textarea className={ui.cx(`${ui.input} min-h-24 resize-y`)} name="terms" required minLength={40} maxLength={6000} />
        </label>
        <div className="flex flex-wrap items-center gap-3 text-[13px] text-muted sm:col-span-2">
          <button className={ui.button()} disabled={create.isPending}>
            {t("Create disabled plan")}
          </button>
          {create.isSuccess ? (
            <span role="status">{t("Database plan saved as disabled.")}</span>
          ) : null}
        </div>
      </form>
    </details>
  );
}

const capacitySchema = z.array(
  z.object({
    id: z.string(),
    name: z.string(),
    revision: z.number().int(),
    ready: z.boolean(),
    pools: z.array(
      z.object({
        node: z.string(),
        poolID: z.string(),
        capacityBytes: z.number(),
        availableBytes: z.number(),
        ready: z.boolean(),
        registered: z.boolean(),
      }),
    ),
  }),
);
const installationSchema = z.array(
  z.object({
    id: z.string(),
    kind: z.string(),
    node: z.string(),
    state: z.string(),
    host: z.string(),
    revision: z.number(),
    observedRevision: z.number(),
  }),
);
function DatabaseCapacity() {
  const { t, error, locale } = useI18n();
  const api = useApi();
  const call = useManagedCommand();
  const client = useQueryClient();
  const [clusterID, setCluster] = useState("");
  const capacity = useQuery({
    queryKey: ["database-capacity"],
    queryFn: async () =>
      (
        await api.write(
          "/api/console/databases",
          "POST",
          { action: "databases.capacity" },
          response(capacitySchema),
        )
      ).data,
    refetchInterval: 30000,
  });
  const cluster =
    capacity.data?.find((c) => c.id === clusterID) ??
    capacity.data?.find((c) => c.ready) ??
    capacity.data?.[0];
  const installations = useQuery({
    queryKey: ["database-management", cluster?.id, "installations"],
    queryFn: () => {
      if (!cluster) throw Error("Choose a cluster.");
      return call(
        { action: "databases.installations.list", clusterID: cluster.id },
        installationSchema,
      );
    },
    enabled: Boolean(cluster),
  });
  const mutation = useMutation({
    mutationFn: (command: unknown) => call(command, z.unknown()),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["database-capacity"] });
      void client.invalidateQueries({ queryKey: ["database-management"] });
    },
  });
  const size = (bytes: number) =>
    new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(
      bytes / 1073741824,
    ) + " GiB";
  return (
    <details className={ui.cx(`database-catalog database-infrastructure ${ui.card} group/details mt-0 pt-0`)}>
      <summary className={summary}>{t("Database infrastructure")}</summary>
      <div className={ui.cx(`${ui.cardBody} border-t border-outline`)}>
      {capacity.isPending ? (
        <Loading />
      ) : capacity.error ? (
        <Failure error={capacity.error} retry={() => void capacity.refetch()} />
      ) : null}
      {mutation.error ? (
        <p role="alert" className="text-sm text-danger">
          {error(mutation.error.message)}
        </p>
      ) : null}
      {capacity.data?.length ? (
        <>
          <label className={ui.cx(`${ui.field} max-w-[420px]`)}>
            {t("Cluster")}
            <select className={ui.input}
              aria-label={t("Cluster")}
              value={cluster?.id ?? ""}
              onChange={(event) => {
                setCluster(event.target.value);
              }}
            >
              {capacity.data.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
          </label>
          {cluster ? (
            <>
              <p>
                {t(
                  cluster.ready
                    ? "A qualified PostgreSQL agent is available."
                    : "Refresh the agent observation before changing database capacity.",
                )}
              </p>
              <button
                className={ui.cx(`${ui.button()} justify-self-start`)}
                onClick={() => void capacity.refetch()}
              >
                {t("Refresh")}
              </button>
              <ul className={list}>
                {cluster.pools.map((pool) => (
                  <li key={pool.poolID} className={ui.cardRow}>
                    <div className="grid min-w-0 gap-0.5">
                      <strong className="font-medium">{pool.node}</strong>
                      <p className="text-[13px] text-muted">
                        {size(pool.availableBytes)} {t("available")} /{" "}
                        {size(pool.capacityBytes)}
                      </p>
                    </div>
                    <button
                      className={ui.button()}
                      disabled={mutation.isPending || !pool.ready}
                      onClick={() => {
                        if (pool.registered) {
                          if (
                            window.confirm(
                              t(
                                "Create a shared PostgreSQL installation on this qualified storage pool?",
                              ),
                            )
                          )
                            mutation.mutate({
                              action: "databases.installations.create",
                              clusterID: cluster.id,
                              node: pool.node,
                              poolID: pool.poolID,
                              profile: "shared",
                              idempotencyKey: key(),
                            });
                        } else
                          mutation.mutate({
                            action: "databases.pools.register",
                            clusterID: cluster.id,
                            node: pool.node,
                            poolID: pool.poolID,
                            revision: cluster.revision,
                            idempotencyKey: key(),
                          });
                      }}
                    >
                      {t(
                        pool.registered
                          ? "Create shared installation"
                          : "Register storage pool",
                      )}
                    </button>
                  </li>
                ))}
              </ul>
              {!cluster.pools.length ? (
                <p>{t("No qualified database volume pool is available.")}</p>
              ) : null}
            </>
          ) : null}
          {installations.error ? <Failure error={installations.error} /> : null}
          {installations.data?.length ? (
            <ul className={list}>
              {installations.data.map((i) => (
                <li key={i.id} className={ui.cardRow}>
                  <div className="grid min-w-0 gap-0.5">
                    <strong className="font-medium [overflow-wrap:anywhere]">{i.host}</strong>
                    <p className="text-[13px] text-muted">
                      {i.node} ·{" "}
                      {t(
                        i.kind === "shared-pool"
                          ? "Shared"
                          : "Dedicated instance",
                      )}{" "}
                      · {t(i.state)}
                    </p>
                  </div>
                  <span className="text-[13px] text-muted">
                    {t("Revision")} {i.observedRevision} / {i.revision}
                  </span>
                </li>
              ))}
            </ul>
          ) : null}
        </>
      ) : capacity.data ? (
        <p className="text-muted">{t("No database clusters are available.")}</p>
      ) : null}
      </div>
    </details>
  );
}
