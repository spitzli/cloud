"use client";
import * as React from "react";
import { useI18n } from "@spitzli/i18n/react";
import {
  formatResource,
  type SetupView,
  type SetupOptions,
} from "@spitzli/hosting-contracts";
export function SetupReview({
  view,
  options,
  accepted,
  onAccept,
}: {
  view: SetupView;
  options: SetupOptions;
  accepted: string[];
  onAccept: (ids: string[]) => void;
}) {
  const { t, error, locale } = useI18n(),
    config = view.configuration,
    preview = view.preview;
  const repository =
    config.source.kind === "git"
      ? options.connections
          .flatMap((c) => c.repositories)
          .find(
            (r) =>
              r.repositoryID ===
              (config.source.kind === "git" ? config.source.repositoryID : ""),
          )
      : null;
  const money = (cents: number, currency: string) =>
    new Intl.NumberFormat(locale, { style: "currency", currency }).format(
      cents / 100,
    );
  return (
    <>
      <h2>{t("Review and deploy")}</h2>
      <dl className="setup-summary">
        <dt>{t("Application")}</dt>
        <dd>{config.name}</dd>
        <dt>{t("Source")}</dt>
        <dd>
          {config.source.kind === "git" ? (
            <>
              {repository?.fullName ?? t("Git repository")}
              <small>
                {config.source.branch} · {config.source.rootDirectory}
              </small>
              {preview?.sourceSHA && (
                <code>{preview.sourceSHA.slice(0, 12)}</code>
              )}
            </>
          ) : config.source.kind === "image" ? (
            <code>{preview?.image ?? config.source.reference}</code>
          ) : (
            t("HTTP healthcheck")
          )}
        </dd>
        <dt>{t("Resources")}</dt>
        <dd>
          {formatResource("cpuMillicores", config.spec.cpuMillicores, locale)}{" "}
          {t("CPU cores")} ·{" "}
          {formatResource("memoryBytes", config.spec.memoryBytes, locale)} ·{" "}
          {config.spec.replicas} {t("Instances")}
        </dd>
        <dt>PostgreSQL</dt>
        <dd>
          {config.database.mode === "none"
            ? t("Not selected")
            : config.database.mode === "new"
              ? config.database.name
              : (options.databases.find(
                  (d) =>
                    d.id ===
                    (config.database.mode === "existing"
                      ? config.database.databaseID
                      : ""),
                )?.name ?? config.database.databaseID)}
          {config.database.mode !== "none" && (
            <small>
              {t("Connection variable")}:{" "}
              <code>{config.database.variable}</code>
            </small>
          )}
        </dd>
        <dt>{t("Mail sending")}</dt>
        <dd>
          {config.mail
            ? (options.mail.find(
                (d) =>
                  d.id === config.mail!.connectionID &&
                  d.domainID === config.mail!.domainID,
              )?.domain ?? t("Selected sender domain"))
            : t("Not selected")}
          {config.mail && (
            <small>
              {t("Mail key variable")}: <code>{config.mail.variable}</code>
            </small>
          )}
        </dd>
        <dt>{t("Environment variables")}</dt>
        <dd>
          {view.environmentNames.length ? (
            <code>{view.environmentNames.join(", ")}</code>
          ) : (
            t("Not selected")
          )}
        </dd>
        <dt>{t("Persistent storage")}</dt>
        <dd>
          {config.spec.volumeBytes
            ? formatResource("volumeBytes", config.spec.volumeBytes, locale) +
              " /data"
            : t("Not selected")}
        </dd>
      </dl>
      <p className="help">
        {t(
          "Your current hosting allowance applies. Existing services keep their current pricing and limits.",
        )}
      </p>
      {!!preview?.blockers.length && (
        <div className="notice error" role="alert">
          <strong>{t("Complete these steps first")}</strong>
          <ul>
            {preview.blockers.map((message) => (
              <li key={message}>{error(message)}</li>
            ))}
          </ul>
        </div>
      )}
      {preview?.offers.map((offer) => (
        <section className="setup-order" key={offer.id}>
          <h3>{offer.name}</h3>
          <p className="setup-price">
            {money(offer.monthlyPriceCents, offer.currency)}{" "}
            <small>{t("net / month")}</small>
          </p>
          <p>{offer.terms}</p>
          <p className="help">
            {t("Backups are not configured. No recovery window is included.")}
          </p>
          <label className="check">
            <input
              type="checkbox"
              name="acceptedOfferIDs"
              value={offer.id}
              checked={accepted.includes(offer.id)}
              onChange={(e) =>
                onAccept(
                  e.target.checked
                    ? [...accepted, offer.id]
                    : accepted.filter((id) => id !== offer.id),
                )
              }
            />
            {t("I accept this monthly price and the displayed terms.")}
          </label>
        </section>
      ))}
      <p className="help">
        {t(
          "Webdock will prepare the selected services, connect their credentials and deploy the application. A failed deployment keeps already provisioned data.",
        )}
      </p>
    </>
  );
}
