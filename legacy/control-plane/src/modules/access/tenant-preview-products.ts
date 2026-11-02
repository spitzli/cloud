import { getTenantPlan } from "../plans/plans";
import type { PreviewProductReader } from "./tenant-preview";
/** Called only after the shared preview checks active scope, customer and read allowlist. */
export const readPreviewProduct: PreviewProductReader = async (operation, headers, customerID) => {
  if (operation === "getTenantMailService") {
      const mail = await (await import("../mail/mail-service")).getTenantMailService(headers, customerID);
      return mail
        ? {
            ...mail,
            canActivate: false,
            canSuspend: false,
            canReconcile: false,
          }
        : null;
    }
  if (operation === "getTenantPlan")
      return getTenantPlan(headers, customerID);
  if (operation === "getTenantStorage")
      return (await import("../platform/storage-usage")).getTenantStorage(headers, customerID);
  // This read uses stored account/domain snapshots and a configured flag only;
  // credential listing and every provider-token operation remain excluded.
  if (operation === "getTenantMail")
      return {
        ...(await (await import("../mail/tenant-mail")).getTenantMail(headers, customerID)),
        operator: false,
        canManage: false,
      };
  throw Error("Unsupported preview read");
};
