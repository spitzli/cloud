import { createHostingSetupRecovery } from "@webdock/module-hosting/server/setup-recovery";
import {
  openEnvironment,
  patchEnvironment,
  sealEnvironment,
} from "./environment";
export const { resetGitSetupRetry, repairSetupEnvironment } =
  createHostingSetupRecovery({
    get openEnvironment() {
      return openEnvironment;
    },
    get patchEnvironment() {
      return patchEnvironment;
    },
    get sealEnvironment() {
      return sealEnvironment;
    },
  });
