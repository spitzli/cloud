import { database } from "../../db/runtime";
import { verifyAgent } from "../hosting/clusters";
import { transaction } from "../hosting/db";
import { createNativeAgent } from "@webdock/module-mail/server/native-agent";
export const {
  mailAgentConfig,
  verifyMailAgent,
  claimNativeMail,
  checkpointNativeMail,
  completeNativeMail,
} = createNativeAgent({
  database,
  verifyAgent,
  transaction,
  env: process.env,
});
