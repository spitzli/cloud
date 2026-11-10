"use client";
import * as React from "react";
import { useState, useContext } from "react";
import { useI18n } from "@spitzli/i18n/react";
import { HostingDirtyContext } from "./form-state";
export function ContainerArguments({ values }: { values: string[] }) {
  const { t } = useI18n(),
    [args, setArgs] = useState(values);
  const markDirty = useContext(HostingDirtyContext);
  return (
    <fieldset>
      <legend>{t("Container arguments")}</legend>
      <input type="hidden" name="args" value={JSON.stringify(args)} />
      {args.map((value, i) => (
        <div className="hosting-argument" key={i}>
          <label>
            {t("Argument")} {i + 1}
            <input
              value={value}
              maxLength={512}
              onChange={(e) =>
                setArgs(args.map((v, j) => (i === j ? e.target.value : v)))
              }
            />
          </label>
          <button
            type="button"
            className="button secondary"
            onClick={() => {
              markDirty();
              setArgs(args.filter((_, j) => i !== j));
            }}
          >
            {t("Remove argument")}
          </button>
        </div>
      ))}
      {args.length < 20 && (
        <button
          className="button secondary"
          type="button"
          onClick={() => {
            markDirty();
            setArgs([...args, ""]);
          }}
        >
          {t("Add argument")}
        </button>
      )}
    </fieldset>
  );
}
