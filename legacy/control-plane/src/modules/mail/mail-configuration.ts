import { clerkMailAccess } from "./mail-clerk-access";
import {
  prepareSubmissionConfiguration,
  completeSubmissionConfiguration,
  reconcileSubmissionConfiguration,
} from "./mail-submission-configuration";
import { mailAgentConfig, verifyMailAgent } from "./mail-agent";
import { transaction } from "../hosting/db";
import { createNativeConfiguration } from "@spitzli/module-mail/server/native-configuration";
export const {
  claimMailConfiguration,
  checkpointMailConfiguration,
  completeMailConfiguration,
} = createNativeConfiguration({
  clerkMailAccess,
  prepareSubmissionConfiguration,
  completeSubmissionConfiguration,
  reconcileSubmissionConfiguration,
  mailAgentConfig,
  verifyMailAgent,
  transaction,
  env: process.env,
});
