"use client";
import { useI18n } from "@spitzli/i18n/react";
import { msgid } from "@spitzli/i18n";
import { useModule } from "@spitzli/module-sdk/react";
import React, { type ReactNode } from "react";
export type StorageConnectionsView = {
  stores: {
    id: string;
    label: string;
    environment: "production" | "preview";
    storeID: string;
    prefix: string;
  }[];
};
/** The caller owns the authenticated server action; only metadata and an in-client form renderer enter here. */
export function StorageConnections({
  storage,
  form,
}: {
  storage: StorageConnectionsView;
  form: (action: string, label: string, children: ReactNode) => ReactNode;
}) {
  const i18n = useI18n();
  const { available, canProvision } = useModule("storage");
  if (!available) return null;
  return (
    <details className="account-section">
      <summary>{i18n.t("Manage storage connections")}</summary>
      <p>
        {i18n.t(
          "Connect each Blob store or a tenant’s exclusive path prefix. Credentials are saved privately and are never displayed. Production and preview storage are measured separately.",
        )}
      </p>
      {canProvision &&
        form(
          "storage-add",
          msgid("Connect storage"),
          <>
            <label className="field">
              {i18n.t("Label")}
              <input name="label" required maxLength={160} />
            </label>
            <label className="field">
              {i18n.t("Environment")}
              <select name="environment">
                <option value="production">{i18n.t("Production")}</option>
                <option value="preview">{i18n.t("Preview")}</option>
              </select>
            </label>
            <label className="field">
              {i18n.t("Blob store ID")}
              <input name="storeID" required maxLength={160} />
            </label>
            <label className="field">
              {i18n.t("Path prefix (optional)")}
              <input name="prefix" maxLength={1024} />
              <span className="help">
                {i18n.t(
                  "Blank measures the entire store. Use an exclusive prefix for a shared store.",
                )}
              </span>
            </label>
            <label className="field">
              {i18n.t("Blob read/write token")}
              <input
                name="token"
                type="password"
                autoComplete="new-password"
                required
              />
            </label>
          </>,
        )}
      {storage.stores.map((store) => (
        <article className="access-record" key={store.id}>
          <h3>{store.label}</h3>
          <p>
            {store.environment} · {store.storeID} ·{" "}
            {store.prefix || i18n.t("Whole store")}
          </p>
          {form(
            "storage-refresh",
            msgid("Refresh measurement"),
            <input type="hidden" name="storageID" value={store.id} />,
          )}
          {form(
            "storage-remove",
            msgid("Disconnect storage"),
            <>
              <input type="hidden" name="storageID" value={store.id} />
              <label className="check">
                <input type="checkbox" required />
                {i18n.t(
                  "Remove this measurement connection. Stored files remain unchanged.",
                )}
              </label>
            </>,
          )}
        </article>
      ))}
    </details>
  );
}
