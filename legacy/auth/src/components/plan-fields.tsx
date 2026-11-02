"use client";
import { HostingAllowanceFields, HostingAllowanceList } from "./hosting-allowances";
import { useI18n } from "@webdock/i18n/react";
import type { Allowances } from "@/lib/plans";

const fields = [
  ["storageBytes", "storageMB", "Storage (MB)", 1_000_000],
  ["mailMessages", "mailMessages", "Mail messages", 1],
  ["transferBytes", "transferGB", "Transfer (GB)", 1_000_000_000],
  ["websites", "websites", "Websites", 1],
  ["editors", "editors", "Editors", 1],
] as const;

export function PlanFields({ values, extras = false }: { values?: Allowances; extras?: boolean }) {
 const { t } = useI18n();
  return <><p className="help">{extras ? t("Blank fields add zero. Extras are added to the assigned plan.") : t("Blank fields are unmetered. Enter 0 for no allocation.")} {t(" MB and GB use decimal units.")}</p>
    {fields.map(([key, name, label, divisor]) => <label className="field" key={key}>{t(label)}<input name={name} type="number" min={0} step={divisor === 1 ? 1 : "any"} defaultValue={values?.[key] == null ? "" : values[key] / divisor} /></label>)}
    <HostingAllowanceFields values={values?.hosting} extras={extras}/>
  </>;
}

export function AllowanceList({ values }: { values: Allowances }) {
 const { t, number: formatNumber } = useI18n();
  return <><dl>{fields.map(([key, , label, divisor]) => <div key={key}><dt>{t(label)}</dt><dd>{values[key] === null ? t("Unmetered") : formatNumber((values[key] / divisor), { maximumFractionDigits: 6 })}</dd></div>)}</dl><HostingAllowanceList values={values.hosting}/></>;
}

export function PlanIdentity({ name = "", description = "" }: { name?: string; description?: string }) {
 const { t } = useI18n();
  return <><label className="field">{t("Name")}<input name="name" required maxLength={160} defaultValue={name} /></label><label className="field">{t("Description")}<textarea name="description" maxLength={2000} defaultValue={description} /></label></>;
}
