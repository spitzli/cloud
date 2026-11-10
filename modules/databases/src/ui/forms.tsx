"use client";
import * as React from "react";

import { useActionState, type ReactNode } from "react";
import { useI18n } from "@spitzli/i18n/react";
import type { DatabaseDirectory } from "@spitzli/database-contracts";
import { useDatabaseActions } from "./actions";

function DatabaseForm({
  children,
  label,
}: {
  children: ReactNode;
  label: string;
}) {
  const { saveDatabase } = useDatabaseActions();
  const { t, error } = useI18n();
  const [state, action, pending] = useActionState(saveDatabase, {});
  return (
    <form action={action} className="hosting-form">
      {children}
      {state.error && (
        <p role="alert" className="notice error">
          {error(state.error)}
        </p>
      )}
      {state.message && <p role="status">{t(state.message)}</p>}
      <button className="button" disabled={pending}>
        {pending ? t("Saving…") : label}
      </button>
    </form>
  );
}

export function AddDatabase({ projects }: Pick<DatabaseDirectory, "projects">) {
  const { t } = useI18n();
  return (
    <details className="panel">
      <summary>{t("Register database")}</summary>
      <DatabaseForm label={t("Register database")}>
        <input type="hidden" name="action" value="create" />
        <label className="field">
          {t("Project")}
          <select name="projectID" required>
            {projects.map((project) => (
              <option key={project.id} value={project.id}>
                {project.name}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          {t("Database engine")}
          <select name="engine">
            <option value="postgresql">PostgreSQL</option>
            <option value="sqlite">SQLite</option>
          </select>
        </label>
        <label className="field">
          {t("Name")}
          <input name="name" required maxLength={120} />
        </label>
        <label className="field">
          {t("Environment")}
          <select name="environment">
            <option value="production">{t("Production")}</option>
            <option value="staging">{t("Staging")}</option>
            <option value="development">{t("Development")}</option>
          </select>
        </label>
      </DatabaseForm>
    </details>
  );
}

export function DatabaseAccess({
  bindingID,
  people,
}: {
  bindingID: string;
  people: DatabaseDirectory["people"];
}) {
  const { t } = useI18n();
  const users = (
    <label className="field">
      {t("Person")}
      <select name="subject" required>
        {people.map((person) => (
          <option key={person.id} value={person.id}>
            {person.name} · {person.email}
          </option>
        ))}
      </select>
    </label>
  );
  return (
    <details className="database-access">
      <summary>{t("Manage database access")}</summary>
      <p className="muted">
        {t(
          "Assign a dedicated Tabularis runtime with a verified database role for this person.",
        )}
      </p>
      <DatabaseForm label={t("Save access")}>
        <input type="hidden" name="action" value="grant" />
        <input type="hidden" name="bindingID" value={bindingID} />
        {users}
        <label className="field">
          {t("Database permission")}
          <select name="profile">
            <option value="read">{t("Read only")}</option>
            <option value="write">{t("Edit data")}</option>
            <option value="schema">{t("Manage schema")}</option>
          </select>
        </label>
        <label className="field">
          {t("Runtime origin")}
          <input
            name="runtimeOrigin"
            type="url"
            required
            placeholder="https://database-runtime.example"
            autoComplete="off"
          />
        </label>
        <label className="field">
          {t("Tabularis connection ID")}
          <input
            name="connectionID"
            required
            maxLength={128}
            autoComplete="off"
          />
        </label>
        <label className="field">
          {t("Runtime proxy secret")}
          <input
            name="proxySecret"
            type="password"
            required
            minLength={32}
            maxLength={512}
            autoComplete="new-password"
          />
        </label>
        <label>
          <input type="checkbox" name="isolationVerified" required />{" "}
          {t(
            "This runtime and its storage are isolated for this person and database.",
          )}
        </label>
        <label>
          <input type="checkbox" name="databaseRoleVerified" required />{" "}
          {t(
            "The database role enforces the selected permission and denies other tenants.",
          )}
        </label>
      </DatabaseForm>
      <details>
        <summary>{t("Revoke access")}</summary>
        <DatabaseForm label={t("Revoke access")}>
          <input type="hidden" name="action" value="revoke" />
          <input type="hidden" name="bindingID" value={bindingID} />
          {users}
        </DatabaseForm>
      </details>
    </details>
  );
}
