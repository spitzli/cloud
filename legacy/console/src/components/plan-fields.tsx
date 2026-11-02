"use client";
import { HostingAllowanceFields, HostingAllowanceList } from "./hosting/allowances";
import { msgid } from '@webdock/i18n';

import { useI18n } from '@webdock/i18n/react';

import type { Allowances } from "@/lib/plans";

const fields = [
  ["storageBytes", "storageMB", msgid("Storage (MB)"), 1_000_000],
  ["mailMessages", "mailMessages", msgid("Mail messages"), 1],
  ["transferBytes", "transferGB", msgid("Transfer (GB)"), 1_000_000_000],
  ["websites", "websites", msgid("Websites"), 1],
  ["editors", "editors", msgid("Editors"), 1],
] as const;

export function PlanFields({
  values,
  extras = false,
}: {
  values?: Allowances;
  extras?: boolean;
}) {
  const i18n = useI18n();

  return (
    <>
      <p className="help">
        {extras
          ? i18n.t("Blank fields add zero. Extras are added to the assigned plan.")
          : i18n.t("Blank fields are unmetered. Enter 0 for no allocation.")}{" "}{i18n.t("MB and GB use decimal units.")}</p>
      <div className="plan-fields">
        {fields.map(([key, name, label, divisor]) => (
          <label className="field" key={key}>
            {i18n.t(label)}
            <input
              name={name}
              type="number"
              min={0}
              step={divisor === 1 ? 1 : "any"}
              defaultValue={values?.[key] == null ? "" : values[key] / divisor}
            />
          </label>
        ))}
      </div>
      <HostingAllowanceFields values={values?.hosting} extras={extras}/>
    </>
  );
}

export function AllowanceList({ values }: { values: Allowances }) {
  const i18n = useI18n();

  return (
    <> <dl className="allowance-list">
      {fields.map(([key, , label, divisor]) => (
        <div key={key}>
          <dt>{i18n.t(label)}</dt>
          <dd>
            {values[key] === null
              ? i18n.t("Unmetered")
              : (values[key] / divisor).toLocaleString(i18n.locale === "de" ? "de-DE" : "en-GB", {
                  maximumFractionDigits: 6,
                })}
          </dd>
        </div>
      ))}
    </dl><HostingAllowanceList values={values.hosting}/></>
  );
}

export function PlanIdentity({
  name = "",
  description = "",
}: {
  name?: string;
  description?: string;
}) {
  const i18n = useI18n();

  return (
    <>
      <label className="field">{i18n.t("Name")}<input name="name" required maxLength={160} defaultValue={name} />
      </label>
      <label className="field">{i18n.t("Description")}<textarea
          name="description"
          maxLength={2000}
          defaultValue={description}
        />
      </label>
    </>
  );
}
