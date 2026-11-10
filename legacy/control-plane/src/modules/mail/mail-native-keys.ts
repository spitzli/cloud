import { authorizeMail } from "./mail-access";
import { createNativeKeys } from "@spitzli/module-mail/server/native-keys";
export { nativeKeyToken } from "@spitzli/module-mail/server/native-keys";
export const {
  requireNativeSending,
  authorizeMailKey,
  authorizeMailKeyID,
  manageNativeMailKey,
} = createNativeKeys({
  authorizeMail,
  env: process.env,
});
