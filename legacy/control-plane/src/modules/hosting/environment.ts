import { createHostingEnvironment } from "@spitzli/module-hosting/server/environment";
import { encryptMailSecret, decryptMailSecret } from "../platform/platform";
export const {
  patchEnvironment,
  openEnvironment,
  sealEnvironment,
  redactValues,
} = createHostingEnvironment({
  get encryptMailSecret() {
    return encryptMailSecret;
  },
  get decryptMailSecret() {
    return decryptMailSecret;
  },
});
