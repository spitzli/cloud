import {
  DatabaseOrderForm,
  type DatabaseCatalogEntry,
} from "@/components/hosting/databases";
import { randomUUID } from "node:crypto";
import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
import {
  formatResource,
  type DatabaseCommand,
} from "@webdock/hosting-contracts";
import { hostingPageCall } from "@/lib/hosting-client";
import { DatabaseCredentialPanel } from "@/components/hosting/database-details";
import { HostingForm } from "@/components/hosting/form";
import { getManagedDatabasePage } from "@/lib/managed-database-page";
import { notFound } from "next/navigation";
export default async function DatabaseDetails({
  params,
  view = "overview",
}: {
  params: Promise<{ databaseID: string }>;
  view?: "overview" | "connection" | "settings";
}) {
  const { databaseID } = await params,
    path = `/hosting/databases/${databaseID}`;
  const { t, locale, date } = await getRequestI18n();
  const { database: d, access } = await getManagedDatabasePage(databaseID);
  if (view !== "overview" && !access.canWrite) notFound();
  const plans =
    view === "settings" && access.canWrite && d.state === "ready"
      ? await hostingPageCall<DatabaseCatalogEntry[]>(
          { action: "databases.plans.list", projectID: d.projectID },
          path,
        )
      : [];
  const upgrades = plans.filter(
    (p) =>
      p.id !== d.planRevisionID &&
      p.availability !== "disabled" &&
      (d.kind === "shared"
        ? p.plan.kind === "dedicated" ||
          p.plan.logicalStorageAllowanceBytes! >=
            d.logicalStorageAllowanceBytes!
        : p.plan.kind === "dedicated" &&
          p.plan.volumeBytes! >= d.volumeBytes! &&
          p.plan.memoryBytes! >= d.memoryBytes! &&
          p.plan.cpuMillicores! >= d.cpuMillicores!),
  );
  const migrations =
    view === "settings" && access.operator
      ? await hostingPageCall<
          Array<{
            id: string;
            state: string;
            sourceBackendID: string;
            retainedUntil: string | null;
          }>
        >({ action: "databases.migrations.list", databaseID: d.id }, path)
      : [];
  // This authenticated server response measures freshness once per request.
  // eslint-disable-next-line react-hooks/purity
  const now = Date.now();
  const observedFresh =
    !!d.observedAt && now - Date.parse(d.observedAt) < 90000;
  const fresh = d.usage && now - Date.parse(d.usage.observedAt) < 90000;
  const actions: Array<{
    action:
      | "pause"
      | "resume"
      | "restart"
      | "rotateCredentials"
      | "requestCancellation"
      | "reconcile";
    label: string;
  }> = [];
  if (["ready", "degraded"].includes(d.state)) {
    actions.push(
      {
        action: "pause",
        label:
          d.kind === "shared"
            ? t("Pause database access")
            : t("Pause instance"),
      },
      { action: "rotateCredentials", label: t("Rotate credentials") },
    );
    if (d.kind === "dedicated")
      actions.push({ action: "restart", label: t("Restart instance") });
  }
  if (d.state === "paused")
    actions.push({ action: "resume", label: t("Resume database") });
  if (!d.cancellationRequestedAt && !["retained", "purged"].includes(d.state))
    actions.push({
      action: "requestCancellation",
      label: t("Request cancellation"),
    });
  if (access.operator && d.state === "needs-reconciliation")
    actions.push({ action: "reconcile", label: t("Reconcile observed state") });
  return (
    <>
      {view === "overview" && (
        <section className="panel">
          {access.canWrite && d.state === "ready" && d.endpoint && <p>
            <Link className="button" href={`${path}/viewer`}>{t("Open in Tabularis")}</Link>
          </p>}
          <dl className="allowance-list">
            <div>
              <dt>{t("Status")}</dt>
              <dd>
                {!observedFresh && ["ready", "degraded"].includes(d.state)
                  ? t("Unknown or stale")
                  : t(d.state)}
              </dd>
            </div>
            <div>
              <dt>{t("Database kind")}</dt>
              <dd>
                {d.kind === "shared" ? t("Shared") : t("Dedicated instance")}
              </dd>
            </div>
            <div>
              <dt>{t("Memory")}</dt>
              <dd>
                {d.memoryBytes === null
                  ? t("Shared resources")
                  : new Intl.NumberFormat(locale, {
                      maximumFractionDigits: 4,
                    }).format(d.memoryBytes / 1073741824) + " GiB"}
              </dd>
            </div>
            <div>
              <dt>{t("Storage allowance")}</dt>
              <dd>
                {new Intl.NumberFormat(locale, {
                  maximumFractionDigits: 4,
                }).format(
                  (d.logicalStorageAllowanceBytes ?? d.volumeBytes ?? 0) /
                    1073741824,
                ) + " GiB"}{" "}
                ·{" "}
                {d.kind === "shared"
                  ? t("Measured logical allowance")
                  : t("Bounded database filesystem")}
              </dd>
            </div>
            <div>
              <dt>{t("Logical data size")}</dt>
              <dd>
                {fresh
                  ? formatResource("volumeBytes", d.usage!.logicalBytes, locale)
                  : t("Unknown or stale")}
              </dd>
            </div>
            {d.kind === "dedicated" && (
              <div>
                <dt>{t("Database filesystem usage")}</dt>
                <dd>
                  {fresh && d.usage!.filesystemBytes !== null
                    ? formatResource(
                        "volumeBytes",
                        d.usage!.filesystemBytes!,
                        locale,
                      )
                    : t("Unknown or stale")}
                </dd>
              </div>
            )}
            <div>
              <dt>{t("Last observation")}</dt>
              <dd>
                {d.observedAt
                  ? date(d.observedAt, {
                      dateStyle: "medium",
                      timeStyle: "short",
                    })
                  : t("Unavailable")}
              </dd>
            </div>
            <div>
              <dt>{t("Backups")}</dt>
              <dd>{t("Not configured")}</dd>
            </div>
          </dl>
          <p>
            {t("Backups are not configured. No recovery window is included.")}
          </p>
          {fresh &&
            d.kind === "shared" &&
            d.usage!.logicalBytes >= d.logicalStorageAllowanceBytes! * 0.7 && (
              <p role="status">
                {t("Your measured storage allowance is nearly reached.")}
              </p>
            )}
          {fresh &&
            d.kind === "dedicated" &&
            d.usage!.filesystemBytes !== null &&
            d.usage!.filesystemBytes! >= d.volumeBytes! * 0.85 && (
              <p role="alert">
                {t(
                  "The database filesystem is nearly full. Review capacity before importing more data.",
                )}
              </p>
            )}
          {d.operationID && (
            <Link href={`/hosting/operations/${d.operationID}`}>
              {t("View progress")}
            </Link>
          )}
          {d.cancellationRequestedAt && (
            <p role="status">
              {t(
                "Cancellation requested. Retained data is deleted separately by an operator.",
              )}
            </p>
          )}
        </section>
      )}
      {view === "connection" && access.canWrite && (
        <DatabaseCredentialPanel databaseID={d.id} />
      )}
      {view === "settings" && access.canWrite && (
        <>
          <section className="panel">
            <h2>{t("Database actions")}</h2>
            <p>
              {t(
                "Pausing interrupts connections and keeps the fixed monthly price. Shared pause affects only your own roles.",
              )}
            </p>
            {actions.map((a) => (
              <HostingForm
                key={a.action}
                label={a.label}
                command={
                  {
                    action: `databases.${a.action}`,
                    databaseID: d.id,
                    revision: d.revision,
                    idempotencyKey: randomUUID(),
                  } as DatabaseCommand
                }
              />
            ))}
          </section>
        </>
      )}
      {view === "settings" &&
        access.canWrite &&
        d.state === "ready" &&
        upgrades.length > 0 && (
          <section className="panel">
            <h2>{t("Upgrade database plan")}</h2>
            <DatabaseOrderForm
              projectID={d.projectID}
              databaseID={d.id}
              databaseName={d.name}
              plans={upgrades}
              requestKey={randomUUID()}
            />
          </section>
        )}
      {view === "settings" && access.operator && migrations.length > 0 && (
        <section className="panel">
          <h2>{t("Retained migration sources")}</h2>
          {migrations.map((m) => (
            <p key={m.id}>
              {m.sourceBackendID} · {t(m.state)}{" "}
              {m.state === "retained" && (
                <Link href={`/hosting/databases/${d.id}/sources/${m.id}`}>
                  {t("Review source cleanup")}
                </Link>
              )}
            </p>
          ))}
        </section>
      )}
      {view === "settings" && access.operator && (
        <Link
          className="button secondary"
          href={`/hosting/databases/${d.id}/deletion`}
        >
          {t("Review database deletion")}
        </Link>
      )}
    </>
  );
}
