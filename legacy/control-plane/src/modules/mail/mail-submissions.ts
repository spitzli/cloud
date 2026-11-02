import { inspectWebmail, inspectQueuedWebmail } from "./mail-sessions";
import { reserveWebmailWrite } from "./mail-writes";
import {
  authorizeMailKey,
  authorizeMailKeyID,
  requireNativeSending,
} from "./mail-native-keys";
import { nativeRelay } from "./mail-send-transport";
import { createNativeSubmissions } from "@webdock/module-mail/server/native-submissions";
export {
  sendEnvelope,
  admissionSchema,
  submissionPolicySchema,
  prepareWebmailSendSchema,
  stageWebmailSendSchema,
} from "@webdock/module-mail/server/native-submissions";
export const {
  preflightSubmission,
  admitSubmission,
  beginSubmission,
  finishSubmission,
  submissionStatus,
  reviewSubmissionPreparation,
  authorizeSubmissionPolicy,
  commitSubmissionPayload,
  abortSubmissionPayload,
  claimSubmission,
  submissionCleanup,
  finishSubmissionCleanup,
  authenticateSubmissionPolicy,
  prepareWebmailSend,
  abortWebmailPreparation,
  stageWebmailSend,
  webmailSendStatus,
  webmailFilingAccess,
  finishWebmailFiling,
  lookupWebmailSend,
} = createNativeSubmissions({
  inspectWebmail,
  inspectQueuedWebmail,
  reserveWebmailWrite,
  authorizeMailKey,
  authorizeMailKeyID,
  requireNativeSending,
  nativeRelay,
  env: process.env,
});
