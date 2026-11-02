import { createHostingSetupCapabilities } from "@webdock/module-hosting/server/setup-capabilities";
import { clusterView } from "./clusters";
export const { applicationCapabilities, setupBuildCapabilities } =
  createHostingSetupCapabilities({
    get clusterView() {
      return clusterView;
    },
    env: process.env,
  });
