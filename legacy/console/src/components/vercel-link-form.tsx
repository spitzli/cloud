"use client";
import { useI18n } from '@webdock/i18n/react';

import { useActionState } from "react";
import { linkVercelProject } from "../lib/vercel-actions";
export function VercelLinkForm({
  projects,
  vercelProjects,
}: {
  projects: { id: string; name: string }[];
  vercelProjects: { id: string; name: string }[];
}) {
  const i18n = useI18n();

  const [state, action, pending] = useActionState(linkVercelProject, {});
  return (
    <form className="editor" action={action} aria-busy={pending}>
      <label className="field">{i18n.t("Studio project")}<select name="project" required defaultValue="">
          <option value="" disabled>{i18n.t("Choose a project")}</option>
          {projects.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
      </label>
      <label className="field">{i18n.t("Vercel project")}<select name="vercelProject" required defaultValue="">
          <option value="" disabled>{i18n.t("Choose an authorized project")}</option>
          {vercelProjects.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
      </label>
      {state.error && (
        <p className="error" role="alert">
          {i18n.error(state.error)}
        </p>
      )}
      {state.message && (
        <p className="success" role="status">
          {i18n.error(state.message, "Changes saved.")}
        </p>
      )}
      <div className="form-footer">
        <button
          className="button"
          disabled={pending || !projects.length || !vercelProjects.length}
        >
          {pending ? i18n.t("Linking…") : i18n.t("Link Vercel project")}
        </button>
      </div>
    </form>
  );
}
