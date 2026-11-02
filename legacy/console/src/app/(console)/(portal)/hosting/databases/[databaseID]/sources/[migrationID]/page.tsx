import { randomUUID } from "node:crypto";
import { notFound } from "next/navigation";
import { getRequestI18n } from "@webdock/i18n/next";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingForm } from "@/components/hosting/form";
export default async function RetainedSource({
  params,
}: {
  params: Promise<{ databaseID: string; migrationID: string }>;
}) {
  const { databaseID, migrationID } = await params,
    path = `/hosting/databases/${databaseID}/sources/${migrationID}`;
  const { t, date } = await getRequestI18n();
  const p = await hostingPageCall<{
    databaseID: string;
    name: string;
    revision: number;
    sourceBackendID: string;
    targetBackendID: string;
    retainedUntil: string | null;
    planHash: string;
  }>({ action: "databases.migrationPurgePreview", migrationID }, path);
  if (p.databaseID !== databaseID) notFound();
  return (
    <section className="panel">
      <h2>{t("Delete retained migration source")}</h2>
      <h2>{p.name}</h2>
      <p>
        {t(
          "This deletes only the retained source copy. The current database remains active.",
        )}
      </p>
      <p>
        {t("Source installation")}: {p.sourceBackendID}
      </p>
      {p.retainedUntil && (
        <p>
          {t("Retained until")}:{" "}
          {date(p.retainedUntil, { dateStyle: "medium", timeStyle: "short" })}
        </p>
      )}
      <HostingForm
        label={t("Delete retained source")}
        command={{
          action: "databases.purgeMigrationSource",
          migrationID,
          revision: p.revision,
          confirmName: "",
          planHash: p.planHash,
          idempotencyKey: randomUUID(),
        }}
      >
        <label className="field">
          {t("Type the exact database name to confirm")}
          <input name="confirmName" required autoComplete="off" />
        </label>
      </HostingForm>
    </section>
  );
}
