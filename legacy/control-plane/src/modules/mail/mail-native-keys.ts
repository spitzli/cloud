import { authorizeMail } from "./mail-access";
import { createNativeKeys } from "@webdock/module-mail/server/native-keys";
export { nativeKeyToken } from "@webdock/module-mail/server/native-keys";
export const {
  requireNativeSending,
  authorizeMailKey,
  authorizeMailKeyID,
  manageNativeMailKey,
} = createNativeKeys({
  authorizeMail,
  env: process.env,
});
