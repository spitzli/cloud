import { transaction } from "../hosting/db";
import { authorizeMail } from "./mail-access";
import { requireNativeSending } from "./mail-native-keys";
import { getPlatformSettings, decryptMailSecret } from "../platform/platform";
import { byokPolicy } from "../hosting/byok";
import { mailProvider } from "../platform/platform";
import { TurboSMTPClient } from "./turbosmtp";
import { createNativeSendTransport } from "@webdock/module-mail/server/native-send-transport";
export const { configureNativeTransport, nativeRelay } =
  createNativeSendTransport({
    transaction,
    authorizeMail,
    requireNativeSending,
    getPlatformSettings,
    decryptMailSecret,
    byokPolicy,
    env: process.env,
    credentialMaster: (customerID) => mailProvider(undefined, customerID),
    credentialIssuer: (token) => new TurboSMTPClient({ apiKey: token }),
  });
