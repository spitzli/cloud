import { browserAuthConfiguration } from "@/lib/browser-auth-config";
import { startBrowserPreview, exitBrowserPreview } from "@/lib/preview-actions";

import { getRequestI18n } from "@webdock/i18n/next";
import type { TenantPreview } from "@/lib/studio-client";
import "./tenant-preview.css";
export async function TenantPreviewStart({
  customerID,
}: {
  customerID: string;
}) {
  const i18n = await getRequestI18n();

  return (
    <form
      method={
        browserAuthConfiguration.provider === "clerk" ? undefined : "post"
      }
      action={
        browserAuthConfiguration.provider === "clerk"
          ? startBrowserPreview
          : "/api/tenant-preview/start"
      }
      className="tenant-preview-start"
    >
      <input type="hidden" name="customerID" value={customerID} />
      <button type="submit" className="button secondary">
        {i18n.t("View as customer")}
      </button>
    </form>
  );
}
export async function TenantPreviewBanner({
  preview,
}: {
  preview: TenantPreview;
}) {
  const i18n = await getRequestI18n();

  return (
    <aside
      className="tenant-preview-banner"
      aria-label={i18n.t("Tenant preview")}
    >
      <div>
        <strong>
          {preview.status === "expired"
            ? i18n.t("Preview expired")
            : i18n.t("Customer view")}
          : {preview.customerName}
        </strong>
        <p>
          {i18n.t("Tenant admin · View only")}
          {preview.status === "active"
            ? i18n.t(" · Until {time}", {
                time: i18n.date(preview.expiresAt, {
                  hour: "2-digit",
                  minute: "2-digit",
                }),
              })
            : i18n.t(" · Please return to your own view.")}
        </p>
      </div>
      <form
        method={
          browserAuthConfiguration.provider === "clerk" ? undefined : "post"
        }
        action={
          browserAuthConfiguration.provider === "clerk"
            ? exitBrowserPreview
            : "/api/tenant-preview/exit"
        }
      >
        <button className="button secondary" type="submit">
          {i18n.t("Return to own view")}
        </button>
      </form>
    </aside>
  );
}
