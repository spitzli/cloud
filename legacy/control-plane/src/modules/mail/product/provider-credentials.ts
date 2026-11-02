import { createMailCredentials } from "@webdock/module-mail/server/provider-credentials";
import { mailDatabase as database, withMailTransaction } from "./store";
import { assertMailAccess } from "./access";
import {
  mailProvider,
  encryptMailSecret,
  decryptMailSecret,
} from "../../platform/platform";
import { TurboSMTPClient } from "../turbosmtp";
const runtime = createMailCredentials({
  database,
  withMailTransaction,
  assertMailAccess,
  encryptMailSecret,
  decryptMailSecret,
  master: (actor) =>
    mailProvider(undefined, actor.customerID, actor.connectionID),
  child: (token) => new TurboSMTPClient({ apiKey: token }),
});
export const {
  ensureManagedSendingCredential,
  resolveSendingCredential,
  mailManagementHeaders,
  rotateManagedSendingCredential,
} = runtime;
