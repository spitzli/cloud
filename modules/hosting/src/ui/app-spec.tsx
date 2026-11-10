import * as React from "react";
import { ContainerArguments } from "./arguments";
import { ApplicationEnvironment } from "./environment";
import { ResourceField } from "@spitzli/hosting-contracts/react";
import type { AppSpec } from "@spitzli/hosting-contracts";
import { getRequestI18n } from "@spitzli/i18n/next";
export async function AppSpecFields({
  spec,
  custom,
  environmentNames,
  storageFixed = false,
}: {
  spec: AppSpec;
  custom: boolean;
  environmentNames?: string[];
  storageFixed?: boolean;
}) {
  const { t } = await getRequestI18n();
  return (
    <>
      <fieldset>
        <legend>{t("Application configuration")}</legend>
        <details>
          <summary>{t("Advanced settings")}</summary>
          <label className="field">
            {t("Template")}
            <select name="template" defaultValue={spec.template}>
              <option value="healthcheck">{t("HTTP healthcheck")}</option>
              {custom && (
                <option value="custom">{t("Custom container")}</option>
              )}
            </select>
          </label>
          {custom && (
            <>
              <label className="field">
                {t("Immutable image (already cached on the node)")}
                <input
                  name="image"
                  defaultValue={spec.image}
                  required
                  maxLength={512}
                />
              </label>
              <ContainerArguments values={spec.args} />
            </>
          )}
          <p>
            {t(
              "The HTTP healthcheck template fixes the image, arguments, port 8080 and /ping path. Custom containers use the values below.",
            )}
          </p>
        </details>
        <div className="plan-fields">
          {(["cpuMillicores", "memoryBytes", "ephemeralBytes"] as const).map(
            (k, i) => (
              <ResourceField
                key={k}
                name={k}
                dimension={k}
                value={spec[k]}
                label={[t("CPU cores"), t("Memory"), t("Temporary storage")][i]}
              />
            ),
          )}
          <label className="field">
            {t("Instances")}
            <input
              name="replicas"
              type="number"
              min="0"
              max="20"
              defaultValue={spec.replicas}
              required
            />
          </label>
          <label className="field">
            {t("HTTP port")}
            <input
              name="port"
              type="number"
              min="1024"
              max="65535"
              defaultValue={spec.port}
              required
            />
          </label>
        </div>
        {storageFixed ? (
          <p>
            {t("Persistent storage size is fixed at application creation.")}
          </p>
        ) : (
          <ResourceField
            name="volumeBytes"
            dimension="volumeBytes"
            value={spec.volumeBytes ?? 0}
            label={t("Persistent storage")}
          />
        )}
        {!storageFixed && (
          <p>
            {t(
              "Optional /data storage: enter 0 to disable or at least 68 MB. One instance only. Filesystem overhead reduces usable space. Data and its allowance remain after application deletion.",
            )}
          </p>
        )}
        <label className="field">
          {t("Healthcheck path")}
          <input
            name="healthPath"
            defaultValue={spec.healthPath}
            required
            maxLength={160}
          />
        </label>
        <p>
          {t(
            "Applications run without root or API tokens, with a read-only filesystem and temporary /tmp storage. Cached images only; no unverified registry pulls.",
          )}
        </p>
      </fieldset>
      <ApplicationEnvironment names={environmentNames} />
    </>
  );
}
