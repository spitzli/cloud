import { createMailStudio } from "@webdock/module-mail/server/product-studio";
import { withAuthenticatedDatabase } from "../../../db/database-context";
import { getTenantTracking, manageTenantTracking } from "../mail-tracking";
import { rotateManagedSendingCredential } from "./provider-credentials";
import { getMailHealth } from "./health";
import { reconcileMail } from "./reconcile";
import { mailDatabase as database } from "./store";
import { getTenant } from "../../access/tenants";
import { authorizeMailSession } from "./access";
import { assertMailAccess } from "./access";
import {
  listTenantMailConnections,
  getMailOnboarding,
  connectByokMail,
  configureManagedMail,
} from "./onboarding";
import { listMailMessages, getMailMessage } from "./messages";
import { getMailAnalytics } from "./analytics";
import { submitMail } from "./sending";
import { cancelScheduledMail, rescheduleMail } from "./schedules";
import {
  getMailDomain,
  claimMailDomain,
  verifyMailDomain,
  removeMailDomain,
} from "./domains";
import { createMailKey, revokeMailKey } from "./api-keys";
import { listMailResource, getMailLog } from "./resource-lists";
import {
  createMailWebhook,
  getMailWebhook,
  updateMailWebhook,
  deleteMailWebhook,
  rotateMailWebhook,
  replayMailWebhook,
  listMailWebhookDeliveries,
} from "./webhooks";
import { setMailSuppression, removeMailSuppression } from "./suppressions";
export { mailStudioShapes } from "@webdock/module-mail/server/product-studio";
export const { dispatchMailStudio } = createMailStudio({
  withAuthenticatedDatabase,
  getTenantTracking,
  manageTenantTracking,
  rotateManagedSendingCredential,
  getMailHealth,
  reconcileMail,
  database,
  getTenant,
  authorizeMailSession,
  assertMailAccess,
  listTenantMailConnections,
  getMailOnboarding,
  connectByokMail,
  configureManagedMail,
  listMailMessages,
  getMailMessage,
  getMailAnalytics,
  submitMail,
  cancelScheduledMail,
  rescheduleMail,
  getMailDomain,
  claimMailDomain,
  verifyMailDomain,
  removeMailDomain,
  createMailKey,
  revokeMailKey,
  listMailResource,
  getMailLog,
  createMailWebhook,
  getMailWebhook,
  updateMailWebhook,
  deleteMailWebhook,
  rotateMailWebhook,
  replayMailWebhook,
  listMailWebhookDeliveries,
  setMailSuppression,
  removeMailSuppression,
});
