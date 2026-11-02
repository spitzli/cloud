import { randomUUID } from "node:crypto";
import { getRequestI18n } from "@webdock/i18n/next";
import type { DatabaseView } from "@webdock/hosting-contracts";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingForm } from "@/components/hosting/form";
export default async function DatabaseDeletion({
  params,
}: {
  params: Promise<{ databaseID: string }>;
}) {
  const { databaseID } = await params,
    path = `/hosting/databases/${databaseID}/deletion`;
  const { t, date } = await getRequestI18n();
  const d = await hostingPageCall<DatabaseView>(
    { action: "databases.get", databaseID },
    path,
  );
  const purge = d.state === "retained";
  const preview = await hostingPageCall<{
    planHash: string;
    revision: number;
    retainedUntil: string | null;
    expiresAt: string;
  }>(
    {
      action: purge ? "databases.purgePreview" : "databases.deletionPreview",
      databaseID,
    },
    path,
  );
  // Server-only hint; the mutation rechecks retention against the database clock.
  // eslint-disable-next-line react-hooks/purity
  const now = Date.now();
  const waiting =
    purge &&
    (!preview.retainedUntil || Date.parse(preview.retainedUntil) > now);
  return (
    <>
      <h2>
        {purge
          ? t("Permanently delete retained database")
          : t("Retain database and end service")}
      </h2>
      <section className="panel">
        <h2>{d.name}</h2>
        <p>
          {d.kind === "shared"
            ? t(
                "This action affects only this logical database and its roles. The shared pool and other customers remain intact.",
              )
            : t("This action affects this dedicated PostgreSQL installation.")}
        </p>
        <p>
          {t(
            "Backups are not configured. Permanent deletion cannot be undone.",
          )}
        </p>
        {preview.retainedUntil && (
          <p>
            {t("Retained until")}:{" "}
            {date(preview.retainedUntil, {
              dateStyle: "medium",
              timeStyle: "short",
            })}
          </p>
        )}
        {waiting ? (
          <p>{t("Retention has not expired.")}</p>
        ) : (
          <HostingForm
            label={
              purge
                ? t("Permanently delete database")
                : t("End service and retain data")
            }
            command={{
              action: purge ? "databases.purge" : "databases.delete",
              databaseID,
              revision: preview.revision,
              confirmName: "",
              planHash: preview.planHash,
              idempotencyKey: randomUUID(),
            }}
          >
            <label className="field">
              {t("Type the exact database name to confirm")}
              <input name="confirmName" required autoComplete="off" />
            </label>
          </HostingForm>
        )}
      </section>
    </>
  );
}
