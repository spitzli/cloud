import { resolveSessionReference } from "../auth/principal";
import { withAuthenticatedDatabase } from "../../db/database-context";
import { queueMailConfiguration } from "./mailboxes";
import { byokPolicy } from "../hosting/byok";
import { transaction } from "../hosting/db";
import { authorizeMail } from "./mail-access";
import { manageNativeMailKey, requireNativeSending } from "./mail-native-keys";
import { configureNativeTransport } from "./mail-send-transport";
import { createNativeSending } from "@webdock/module-mail/server/native-sending";
export const { getNativeSending, manageNativeSending } = createNativeSending({
  resolveSessionReference,
  withAuthenticatedDatabase,
  queueMailConfiguration,
  byokPolicy,
  transaction,
  authorizeMail,
  manageNativeMailKey,
  requireNativeSending,
  configureNativeTransport,
  env: process.env,
});
