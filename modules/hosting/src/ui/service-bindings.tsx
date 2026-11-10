"use client";
import * as React from "react";
import type { ComponentProps } from "react";
import { hostingUIScope } from "./scope";
import { useState } from "react";
import { useI18n } from "@spitzli/i18n/react";
import type {
  SetupOptions,
  ServiceSelection,
} from "@spitzli/hosting-contracts";
import { HostingForm } from "./form";
function ConnectServiceState({
  options,
  requestKey,
  appID,
}: {
  options: SetupOptions;
  requestKey: string;
  appID?: string;
}) {
  const { t } = useI18n();
  const [selectedApp, setApp] = useState(
    appID ?? options.applications[0]?.id ?? "",
  );
  const [kind, setKind] = useState<"postgres" | "mail">("postgres"),
    [variable, setVariable] = useState("DATABASE_URL"),
    [key, setKey] = useState(requestKey);
  const databases = options.databases.filter((d) => d.state === "ready"),
    domains = options.mail.filter(
      (d) =>
        d.state === "ready" && d.status === "ownership_verified" && d.domainID,
    );
  const [databaseID, setDatabase] = useState(databases[0]?.id ?? ""),
    [mail, setMail] = useState(
      domains[0] ? domains[0].id + ":" + domains[0].domainID : "",
    );
  const [connectionID, domainID] = mail.split(":");
  const service: ServiceSelection =
    kind === "postgres"
      ? { kind, databaseID, variable }
      : { kind, connectionID, domainID, variable };
  const valid =
    !!selectedApp &&
    !!variable &&
    (kind === "postgres" ? !!databaseID : !!connectionID && !!domainID);
  return (
    <HostingForm
      disabled={!valid}
      command={{
        action: "services.bind",
        projectID: options.project.projectID,
        appID: selectedApp || "1",
        service,
        revision: 0,
        idempotencyKey: key,
      }}
      label={t("Connect service")}
    >
      <fieldset
        className="setup-fields-root"
        onChange={() => setKey(crypto.randomUUID())}
      >
        {!appID && (
          <label className="field">
            {t("Application")}
            <select
              value={selectedApp}
              onChange={(e) => setApp(e.target.value)}
            >
              {options.applications.map((a) => (
                <option value={a.id} key={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
          </label>
        )}
        <label className="field">
          {t("Service")}
          <select
            value={kind}
            onChange={(e) => {
              const kind = e.target.value as "postgres" | "mail";
              setKind(kind);
              setVariable(
                kind === "postgres" ? "DATABASE_URL" : "WEBDOCK_MAIL_API_KEY",
              );
            }}
          >
            <option value="postgres">PostgreSQL</option>
            <option value="mail">{t("Mail sending")}</option>
          </select>
        </label>
        {kind === "postgres" ? (
          <label className="field">
            {t("Database")}
            <select
              required
              value={databaseID}
              onChange={(e) => setDatabase(e.target.value)}
            >
              <option value="">{t("Choose a database")}</option>
              {databases.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.name}
                </option>
              ))}
            </select>
          </label>
        ) : (
          <label className="field">
            {t("Sender domain")}
            <select
              required
              value={mail}
              onChange={(e) => setMail(e.target.value)}
            >
              <option value="">{t("Choose a sender domain")}</option>
              {domains.map((d) => (
                <option
                  key={d.id + ":" + d.domainID}
                  value={d.id + ":" + d.domainID}
                >
                  {d.domain}
                </option>
              ))}
            </select>
          </label>
        )}
        <label className="field">
          {t("Environment variable")}
          <input
            required
            value={variable}
            pattern="[A-Za-z_][A-Za-z0-9_]*"
            maxLength={128}
            onChange={(e) => setVariable(e.target.value)}
          />
        </label>
        {!valid && (
          <p className="help">
            {t("Choose an application and a ready service before connecting.")}
          </p>
        )}
      </fieldset>
    </HostingForm>
  );
}

export function ConnectService(
  props: ComponentProps<typeof ConnectServiceState>,
) {
  return <ConnectServiceState key={hostingUIScope(props)} {...props} />;
}
