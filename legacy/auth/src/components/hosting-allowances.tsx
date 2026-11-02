"use client";
import { ResourceField } from "@webdock/hosting-contracts/react";
import {formatResource} from "@webdock/hosting-contracts";
import { msgid } from "@webdock/i18n";
import { useI18n } from "@webdock/i18n/react";
import {
  hostingDimensions,
  normalizeHostingAllowances,
  type HostingAllowances,
} from "@webdock/hosting-contracts";
const labels = {
  apps: msgid("Hosting apps"),
  cpuMillicores: msgid("CPU cores"),
  memoryBytes: msgid("Memory"),
  volumeBytes: msgid("Persistent storage"),
  ephemeralBytes: msgid("Temporary storage"),
  replicasPerApp: msgid("Replicas per app"),
  concurrentDeployments: msgid("Concurrent deployments"),
};
export function HostingAllowanceFields({
  values,
  extras = false,
}: {
  values?: HostingAllowances;
  extras?: boolean;
}) {
  const { t } = useI18n();
  const data = normalizeHostingAllowances(values);
  return (
    <fieldset>
      <legend>{t("Hosting allowances")}</legend>
      <p className="help">{t("Set resources in cores, MB or GB. Existing applications keep their allocation.")}</p>
      <div className="plan-fields">
        {hostingDimensions.map((k) => (
          <ResourceField key={k} name={`hosting.${k}`} dimension={k} value={data[k]} unlimited={!extras} label={t(labels[k])} />
        ))}
      </div>
    </fieldset>
  );
}
export function HostingAllowanceList({
  values,
}: {
  values?: HostingAllowances;
}) {
  const { t, locale } = useI18n();
  const data = normalizeHostingAllowances(values);
  return (
    <dl className="allowance-list">
      {hostingDimensions.map((k) => (
        <div key={k}>
          <dt>{t(labels[k])}</dt>
          <dd>{data[k] === null ? t("Unlimited") : formatResource(k,data[k],locale)}</dd>
        </div>
      ))}
    </dl>
  );
}
