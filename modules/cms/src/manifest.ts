import type { ModuleManifest } from "@spitzli/module-sdk";
export const cmsManifest = {
  id: "cms",
  version: "0.1.0",
  sdkVersion: 1,
  dependencies: [],
  navigation: [
    {
      slot: "tenant.services",
      id: "cms",
      label: "CMS",
      path: "/cms",
    },
  ],
} as const satisfies ModuleManifest;
