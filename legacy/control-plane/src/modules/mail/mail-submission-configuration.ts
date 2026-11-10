import { nativeRelay } from "./mail-send-transport";
import { createNativeSubmissionConfiguration } from "@spitzli/module-mail/server/native-submission-configuration";
export {
  submissionResources,
  submissionConfigurationProof,
  type SubmissionDesired,
} from "@spitzli/module-mail/server/native-submission-configuration";
export const {
  prepareSubmissionConfiguration,
  completeSubmissionConfiguration,
  reconcileSubmissionConfiguration,
} = createNativeSubmissionConfiguration({
  nativeRelay,
  env: process.env,
});
