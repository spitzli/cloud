"use client";
import {msgid} from '@webdock/i18n';
import { useI18n } from '@webdock/i18n/react';

import { Fragment, useActionState, useId } from "react";
import type { FormState } from "../lib/actions";
export type EditorField = {
  name: string;
  label: string;
  type?: "text" | "email" | "tel" | "url" | "textarea" | "select" | "checkbox";
  required?: boolean;
  value?: string;
  options?: { value: string; label: string; translate?: boolean }[];
  hint?: string;
  group?: string;
  maxLength?: number;
  readOnly?: boolean;
};
export function Editor({
  action,
  fields,
  submit = msgid("Save changes"),
}: {
  action: (state: FormState, form: FormData) => Promise<FormState>;
  fields: EditorField[];
  submit?: string;
}) {
  const i18n = useI18n();

  const [state, formAction, pending] = useActionState(action, {});
  const formID = useId();
  return (
    <form action={formAction} className="editor" aria-busy={pending}>
      <p className="form-instructions">{i18n.t("Fields marked * are required.")}</p>
      {state.error && (
        <p role="alert" className="error">
          {i18n.error(state.error)}
        </p>
      )}
      {state.success && (
        <p role="status" className="success">
          {i18n.error(state.success, "Changes saved.")}
        </p>
      )}
      {fields.map((f) => (
        <Fragment key={f.name}>
          {f.group && <h3 className="editor-section">{i18n.t(f.group)}</h3>}
          <div
            className={"field " + (f.type === "textarea" ? "wide" : "")}
          >
            <label htmlFor={`${formID}-${f.name}`}>
              {i18n.t(f.label)}
              {f.required && <span aria-hidden="true"> *</span>}
            </label>
            {f.type === "textarea" ? (
              <textarea
                id={`${formID}-${f.name}`}
                aria-describedby={f.hint ? `${formID}-${f.name}-hint` : undefined}
                name={f.name}
                rows={4}
                required={f.required}
                readOnly={f.readOnly}
                maxLength={3000}
                defaultValue={state.values?.[f.name] ?? f.value}
              />
            ) : f.type === "select" ? (
              <select
                id={`${formID}-${f.name}`}
                aria-describedby={f.hint ? `${formID}-${f.name}-hint` : undefined}
                name={f.name}
                required={f.required}
                defaultValue={state.values?.[f.name] ?? f.value ?? ""}
              >
                {f.options?.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.translate === false ? o.label : i18n.t(o.label)}
                  </option>
                ))}
              </select>
            ) : f.type === "checkbox" ? (
              <input
                id={`${formID}-${f.name}`}
                aria-describedby={f.hint ? `${formID}-${f.name}-hint` : undefined}
                name={f.name}
                type="checkbox"
                value="yes"
                required={f.required}
                defaultChecked={(state.values?.[f.name] ?? f.value) === "yes"}
              />
            ) : (
              <input
                id={`${formID}-${f.name}`}
                aria-describedby={f.hint ? `${formID}-${f.name}-hint` : undefined}
                name={f.name}
                type={f.type || "text"}
                required={f.required}
                readOnly={f.readOnly}
                maxLength={f.maxLength ?? (f.type === "url" ? 2048 : 160)}
                defaultValue={state.values?.[f.name] ?? f.value ?? ""}
              />
            )}
            {f.hint && <small id={`${formID}-${f.name}-hint`}>{i18n.t(f.hint)}</small>}
          </div>
        </Fragment>
      ))}
      <div className="form-footer">
        <button className="button" disabled={pending}>
          {pending ? i18n.t("Saving…") : i18n.t(submit)}
        </button>
      </div>
    </form>
  );
}
export function ArchiveButton({
  action,
  archived,
}: {
  action: (state: FormState, form: FormData) => Promise<FormState>;
  archived: boolean;
}) {
  const i18n = useI18n();

  const [state, formAction, pending] = useActionState(action, {});
  return (
    <form action={formAction}>
      <input type="hidden" name="restore" value={String(archived)} />
      <details className="archive-controls" key={String(archived)}>
        <summary>{archived ? i18n.t("Restore record") : i18n.t("Archive record")}</summary>
        <p>
          {archived
            ? i18n.t("Return this record to your active workspace.")
            : i18n.t("Keep this record and its history, and remove it from the active list. You can restore it later.")}
        </p>
        <button className="button secondary" disabled={pending}>
          {pending
            ? i18n.t("Saving…")
            : archived
              ? i18n.t("Confirm restore")
              : i18n.t("Confirm archive")}
        </button>
      </details>
      {state.error && (
        <p role="alert" className="error">
          {i18n.error(state.error)}
        </p>
      )}
    </form>
  );
}
