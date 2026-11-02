import { createMailApi } from "@webdock/module-mail/server/product-api";
import { mailEndpoint } from "./http";
import { submitMail } from "./sending";
import { submitMailBatch, getMailBatch } from "./batches";
import { listMailMessages, getMailMessage } from "./messages";
import { getMailAnalytics } from "./analytics";
import { rescheduleMail, cancelScheduledMail } from "./schedules";
import { createMailKey, revokeMailKey } from "./api-keys";
import {
  claimMailDomain,
  getMailDomain,
  verifyMailDomain,
  removeMailDomain,
} from "./domains";
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
import { listMailResource, getMailLog } from "./resource-lists";
import { mailDatabase as database } from "./store";
export const { mailRoute } = createMailApi({
  mailEndpoint,
  submitMail,
  submitMailBatch,
  getMailBatch,
  listMailMessages,
  getMailMessage,
  getMailAnalytics,
  rescheduleMail,
  cancelScheduledMail,
  createMailKey,
  revokeMailKey,
  claimMailDomain,
  getMailDomain,
  verifyMailDomain,
  removeMailDomain,
  createMailWebhook,
  getMailWebhook,
  updateMailWebhook,
  deleteMailWebhook,
  rotateMailWebhook,
  replayMailWebhook,
  listMailWebhookDeliveries,
  setMailSuppression,
  removeMailSuppression,
  listMailResource,
  getMailLog,
  database,
});
