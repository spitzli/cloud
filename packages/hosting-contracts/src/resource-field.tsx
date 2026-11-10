"use client";
import React, { useId, useState } from "react";
import { useI18n } from "@spitzli/i18n/react";
import { parseResourceInput, resourceInput, resourceInUnit } from "./units";
export function ResourceField({
  name,
  dimension,
  value,
  unlimited = false,
  label,
}: {
  name: string;
  dimension: string;
  value: number | null;
  unlimited?: boolean;
  label: string;
}) {
  const { t, locale } = useI18n(),
    id = useId();
  const [mode, setMode] = useState(
    value === null ? "unlimited" : value === 0 ? "disabled" : "limited",
  );
  const initial = resourceInput(dimension, value ?? 0).split(" ");
  const [amount, setAmount] = useState(
    initial[0].replace(".", locale === "de" ? "," : "."),
  );
  const [unit, setUnit] = useState(initial[1] ?? "");
  const [error, setError] = useState(false);
  const limited = !unlimited || mode === "limited";
  const serialized =
    mode === "unlimited" && unlimited
      ? "unlimited"
      : !limited
        ? "0"
        : amount + (unit ? " " + unit : "");
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      {unlimited && (
        <select
          aria-label={label + " — " + t("Limit")}
          value={mode}
          onChange={(e) => setMode(e.target.value)}
        >
          <option value="limited">{t("Set limit")}</option>
          <option value="disabled">{t("Disabled")}</option>
          <option value="unlimited">{t("Unlimited")}</option>
        </select>
      )}
      {limited && (
        <div style={{ display: "flex", gap: ".5rem" }}>
          <input
            id={id}
            value={amount}
            inputMode="decimal"
            required
            maxLength={40}
            aria-invalid={error}
            onChange={(e) => {
              setAmount(e.target.value);
              try {
                parseResourceInput(
                  dimension,
                  e.target.value + (unit ? " " + unit : ""),
                );
                setError(false);
                e.target.setCustomValidity("");
              } catch {
                setError(true);
                e.target.setCustomValidity(t("Enter a valid resource amount."));
              }
            }}
          />
          {dimension.endsWith("Bytes") ? (
            <select
              aria-label={label + " — " + t("Unit")}
              value={unit}
              onChange={(e) => {
                try {
                  const original = parseResourceInput(
                    dimension,
                    amount + " " + unit,
                  );
                  setAmount(
                    resourceInUnit(dimension, original, e.target.value).replace(
                      ".",
                      locale === "de" ? "," : ".",
                    ),
                  );
                  setUnit(e.target.value);
                  setError(false);
                } catch {
                  setError(true);
                }
              }}
            >
              {["MB", "GB", "TB"].map((u) => (
                <option key={u}>{u}</option>
              ))}
            </select>
          ) : dimension === "cpuMillicores" ? (
            <span>{t("cores")}</span>
          ) : null}
        </div>
      )}
      <input type="hidden" name={name} value={serialized} />
    </div>
  );
}
