import { useMemo, useState } from "react";
import { useI18n } from "@webdock/i18n/react";
import { countryOptions } from "./countries";
import { FieldError } from "./live-validation";
import type { RegistryRecord } from "./api";
import * as ui from "./ui";
// Shared by the operator registry and the customer's own onboarding.
export const customerFields = [
  { key: "firstName", label: "First name", autoComplete: "given-name" },
  { key: "lastName", label: "Last name", autoComplete: "family-name" },
  { key: "companyName", label: "Company name", autoComplete: "organization" },
  { key: "contactName", label: "Contact name", autoComplete: "name" },
  {
    key: "addressLine1",
    label: "Address line 1",
    autoComplete: "address-line1",
  },
  {
    key: "addressLine2",
    label: "Address line 2",
    autoComplete: "address-line2",
  },
  {
    key: "postalCode",
    label: "Postal code",
    autoComplete: "postal-code",
    max: 32,
  },
  { key: "city", label: "City", autoComplete: "address-level2" },
  { key: "region", label: "Region", autoComplete: "address-level1" },
  { key: "country", label: "Country code", autoComplete: "country", max: 2 },
] as const;
export function CustomerFields({
  record,
  pending,
  errors,
}: {
  record: RegistryRecord | undefined;
  pending: boolean;
  errors: Record<string, string>;
}) {
  const i18n = useI18n();
  const [type, setType] = useState(record?.customerType === "person" ? "person" : "company");
  const countries = useMemo(() => countryOptions(i18n.locale), [i18n.locale]);
  const visible = type === "company" ? ["companyName", "contactName"] : ["firstName", "lastName"];
  const field = (key: (typeof customerFields)[number]["key"], label: string, autoComplete: string, extra: { required?: boolean; max?: number } = {}) => (
    <div key={key} className="grid content-start gap-1.5">
      <label className={ui.field}>
        {label}
        <input className={ui.input} name={key} maxLength={extra.max ?? 160} required={extra.required} autoComplete={autoComplete} defaultValue={record?.[key] ?? ""} disabled={pending} />
      </label>
      <FieldError className="" message={errors[key]} />
    </div>
  );
  return (
    <>
      <fieldset className="grid min-w-0 border-0 p-0 gap-3">
        <legend className={ui.legend}>Customer type</legend>
        <div className="inline-flex justify-self-start rounded-lg bg-soft p-0.5" role="radiogroup" aria-label="Customer type">
          {(["company", "person"] as const).map((value) => (
            <label key={value} className={`cursor-pointer rounded-md px-3 py-1 text-[13px] font-medium has-focus-visible:outline-2 has-focus-visible:outline-focus ${type === value ? "bg-surface text-foreground shadow-sm" : "text-muted hover:text-foreground"}`}>
              <input type="radio" name="customerType" value={value} checked={type === value} onChange={() => { setType(value); }} disabled={pending} className="sr-only" />
              {value === "company" ? "Company" : "Person"}
            </label>
          ))}
        </div>
        <div className="grid gap-3 sm:grid-cols-2">
          {type === "company"
            ? [field("companyName", "Company name", "organization", { required: true }), field("contactName", "Contact person", "name")]
            : [field("firstName", "First name", "given-name", { required: true }), field("lastName", "Last name", "family-name", { required: true })]}
        </div>
        {customerFields.filter((item) => ["companyName", "contactName", "firstName", "lastName"].includes(item.key) && !visible.includes(item.key)).map((item) => (
          // Values of the other customer type are kept, not silently cleared.
          <input key={item.key} type="hidden" name={item.key} defaultValue={record?.[item.key] ?? ""} />
        ))}
      </fieldset>
      <fieldset className="grid min-w-0 border-0 p-0 gap-3 sm:grid-cols-2">
        <legend className={ui.legend}>Contact</legend>
        <div className="grid content-start gap-1.5">
          <label className={ui.field}>
            Contact email
            <input className={ui.input} type="email" name="contactEmail" maxLength={254} defaultValue={record?.contactEmail ?? ""} autoComplete="email" disabled={pending} />
          </label>
          <FieldError className="" message={errors.contactEmail} />
        </div>
        <div className="grid content-start gap-1.5">
          <label className={ui.field}>
            Phone
            <input className={ui.input} type="tel" name="phone" maxLength={50} pattern="[+0-9 ()/-]{3,50}" title="Digits, spaces and + ( ) / - only" defaultValue={record?.phone ?? ""} autoComplete="tel" disabled={pending} />
          </label>
          <FieldError className="" message={errors.phone} />
        </div>
      </fieldset>
      <fieldset className="grid min-w-0 border-0 p-0 gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,2fr)]">
        <legend className={ui.legend}>Address</legend>
        <div className="sm:col-span-2">{field("addressLine1", "Street and number", "address-line1")}</div>
        <div className="sm:col-span-2">{field("addressLine2", "Address line 2 (optional)", "address-line2")}</div>
        {field("postalCode", "Postal code", "postal-code", { max: 32 })}
        {field("city", "City", "address-level2")}
        {field("region", "State or region (optional)", "address-level1")}
        <label className={ui.field}>
          Country
          <select className={ui.input} name="country" defaultValue={record?.country ?? ""} autoComplete="country" disabled={pending}>
            <option value="">Choose a country</option>
            {countries.map((country) => <option key={country.code} value={country.code}>{country.name}</option>)}
          </select>
        </label>
      </fieldset>
    </>
  );
}
