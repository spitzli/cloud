"use client";
import * as React from "react";
import { resourceScope as hostingUIScope } from "@webdock/console-kit/resource-scope";
import { useState, useTransition } from "react";
import { useI18n } from "@webdock/i18n/react";
import type { DatabaseCredentials } from "../ui-types";
import { useDatabaseActions } from "./actions";
export function DatabaseCredentialPanelWithActions({ databaseID, revealDatabaseCredentials }: { databaseID: string; revealDatabaseCredentials: ReturnType<typeof useDatabaseActions>["revealDatabaseCredentials"] }): React.JSX.Element {

  const { t, error } = useI18n();
  const [value, setValue] = useState<DatabaseCredentials | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [pending, start] = useTransition();
  return (
    <section className="panel">
      <h2>{t("Connection credentials")}</h2>
      <p>
        {t(
          "Use the direct port for migrations and the pooled port for application connections. Always verify TLS.",
        )}
      </p>
      <p>
        {t("Application schema")}: <code>app</code>
      </p>
      {failure && <p role="alert">{error(failure)}</p>}
      {value ? (
        <>
          <dl className="allowance-list">
            <div>
              <dt>{t("Host")}</dt>
              <dd>{value.host}</dd>
            </div>
            <div>
              <dt>{t("Database")}</dt>
              <dd>{value.databaseName}</dd>
            </div>
            <div>
              <dt>{t("Direct / pooled port")}</dt>
              <dd>
                {value.directPort} / {value.pooledPort}
              </dd>
            </div>
          </dl>
          {(["application", "migration"] as const).map((kind) => (
            <fieldset key={kind}>
              <legend>
                {kind === "application"
                  ? t("Application role")
                  : t("Migration role")}
              </legend>
              <label className="field">
                {t("Username")}
                <input readOnly value={value[kind].username} />
              </label>
              <label className="field">
                {t("Password")}
                <input
                  type="password"
                  readOnly
                  value={value[kind].password}
                  autoComplete="off"
                />
              </label>
              <button
                className="button secondary"
                type="button"
                onClick={() =>
                  void navigator.clipboard
                    .writeText(value[kind].password)
                    .catch(() => setFailure("Clipboard access failed."))
                }
              >
                {t("Copy password")}
              </button>
            </fieldset>
          ))}
          <p>
            <code>sslmode=verify-full sslrootcert=webdock-database-ca.pem</code>
          </p>
          <button
            type="button"
            className="button secondary"
            onClick={() => {
              const url = URL.createObjectURL(
                new Blob([value.caCertificate], {
                  type: "application/x-pem-file",
                }),
              );
              const link = document.createElement("a");
              link.href = url;
              link.download = "webdock-database-ca.pem";
              link.click();
              URL.revokeObjectURL(url);
            }}
          >
            {t("Download CA certificate")}
          </button>
          <button
            type="button"
            className="button secondary"
            onClick={() => setValue(null)}
          >
            {t("Hide credentials")}
          </button>
        </>
      ) : (
        <button
          type="button"
          className="button"
          disabled={pending}
          onClick={() =>
            start(async () => {
              const result = await revealDatabaseCredentials(databaseID);
              setFailure(result.error ?? null);
              setValue(result.credentials ?? null);
            })
          }
        >
          {pending ? t("Loading…") : t("Reveal credentials")}
        </button>
      )}
    </section>
  );
}

export function DatabaseCredentialPanel(
  props: { databaseID: string },
) {
  const { revealDatabaseCredentials } = useDatabaseActions();
  return <DatabaseCredentialPanelWithActions key={hostingUIScope(props)} {...props} revealDatabaseCredentials={revealDatabaseCredentials} />;
}
