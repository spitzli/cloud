"use client";
import * as React from "react";
import { useI18n } from "@spitzli/i18n/react";
import type {
  SetupConfiguration,
  SetupOptions,
} from "@spitzli/hosting-contracts";
export function SetupServiceFields({
  configuration,
  options,
  onChange,
}: {
  configuration: SetupConfiguration;
  options: SetupOptions;
  onChange: (value: SetupConfiguration) => void;
}) {
  const { t, locale } = useI18n();
  const database = configuration.database;
  const plans = options.plans.filter((p) => p.availability !== "disabled");
  const domains = options.mail.filter(
    (m) =>
      m.state === "ready" && m.status === "ownership_verified" && m.domainID,
  );
  const money = (value: number) =>
    new Intl.NumberFormat(locale, {
      style: "currency",
      currency: "EUR",
    }).format(value / 100);
  return (
    <>
      <h2>{t("What does your application need?")}</h2>
      <p className="muted">
        {t(
          "Choose services now. Webdock will connect them to the application securely.",
        )}
      </p>
      <fieldset className="setup-service">
        <legend>PostgreSQL</legend>
        <label className="field">
          {t("Database")}
          <select
            value={database.mode}
            onChange={(e) =>
              onChange({
                ...configuration,
                database:
                  e.target.value === "none"
                    ? { mode: "none" }
                    : e.target.value === "existing"
                      ? {
                          mode: "existing",
                          databaseID:
                            options.databases.find((d) => d.state === "ready")
                              ?.id ?? "",
                          variable: "DATABASE_URL",
                        }
                      : {
                          mode: "new",
                          name: configuration.name + "-db",
                          planRevisionID: plans[0]?.id ?? "",
                          variable: "DATABASE_URL",
                        },
              })
            }
          >
            <option value="none">{t("No database")}</option>
            <option value="new" disabled={!plans.length}>
              {t("Create a database")}
            </option>
            <option
              value="existing"
              disabled={!options.databases.some((d) => d.state === "ready")}
            >
              {t("Connect an existing database")}
            </option>
          </select>
        </label>
        {database.mode === "new" && (
          <div className="setup-fields">
            <label className="field">
              {t("Database name")}
              <input
                required
                value={database.name}
                onChange={(e) =>
                  onChange({
                    ...configuration,
                    database: { ...database, name: e.target.value },
                  })
                }
              />
            </label>
            <label className="field">
              {t("Database plan")}
              <select
                required
                value={database.planRevisionID}
                onChange={(e) =>
                  onChange({
                    ...configuration,
                    database: { ...database, planRevisionID: e.target.value },
                  })
                }
              >
                {plans.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.plan.name} — {money(p.plan.monthlyPriceCents)}{" "}
                    {t("net / month")}
                  </option>
                ))}
              </select>
            </label>
            <p className="help">
              {t(
                "You will review and explicitly accept the price before anything is ordered.",
              )}
            </p>
          </div>
        )}
        {database.mode === "existing" && (
          <label className="field">
            {t("Existing database")}
            <select
              required
              value={database.databaseID}
              onChange={(e) =>
                onChange({
                  ...configuration,
                  database: { ...database, databaseID: e.target.value },
                })
              }
            >
              {options.databases
                .filter((d) => d.state === "ready")
                .map((d) => (
                  <option key={d.id} value={d.id}>
                    {d.name}
                  </option>
                ))}
            </select>
          </label>
        )}
        {database.mode !== "none" && (
          <label className="field">
            {t("Connection variable")}
            <input
              required
              value={database.variable}
              onChange={(e) =>
                onChange({
                  ...configuration,
                  database: { ...database, variable: e.target.value },
                })
              }
            />
            <small>
              {t(
                "Webdock supplies the application credentials and verified TLS connection.",
              )}
            </small>
          </label>
        )}
        {!plans.length && !options.databases.length && (
          <p className="help">{t("No database plans are available yet.")}</p>
        )}
      </fieldset>
      <fieldset className="setup-service">
        <legend>{t("Mail sending")}</legend>
        <label className="check">
          <input
            type="checkbox"
            checked={!!configuration.mail}
            disabled={!domains.length}
            onChange={(e) =>
              onChange({
                ...configuration,
                mail: e.target.checked
                  ? {
                      connectionID: domains[0].id,
                      domainID: domains[0].domainID!,
                      variable: "SPITZLI_MAIL_API_KEY",
                    }
                  : null,
              })
            }
          />
          {t("Connect native Mail sending")}
        </label>
        {!domains.length && (
          <p className="help">
            {t(
              "Configure a sending connection and verify its domain below, then refresh the available services.",
            )}
          </p>
        )}
        {configuration.mail && (
          <div className="setup-fields">
            <label className="field">
              {t("Sender domain")}
              <select
                value={
                  configuration.mail.connectionID +
                  ":" +
                  configuration.mail.domainID
                }
                onChange={(e) => {
                  const [connectionID, domainID] = e.target.value.split(":");
                  onChange({
                    ...configuration,
                    mail: { ...configuration.mail!, connectionID, domainID },
                  });
                }}
              >
                {domains.map((d) => (
                  <option
                    key={d.id + ":" + d.domainID}
                    value={d.id + ":" + d.domainID}
                  >
                    {d.domain} (
                    {d.kind === "platform"
                      ? t("Managed Mail")
                      : t("Own provider")}
                    )
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              {t("Mail key variable")}
              <input
                required
                value={configuration.mail.variable}
                onChange={(e) =>
                  onChange({
                    ...configuration,
                    mail: { ...configuration.mail!, variable: e.target.value },
                  })
                }
              />
            </label>
          </div>
        )}
        <p className="help">
          {t(
            "Sending uses a domain-restricted API key. Mailboxes and forwarding are activated separately.",
          )}
        </p>
      </fieldset>
    </>
  );
}
