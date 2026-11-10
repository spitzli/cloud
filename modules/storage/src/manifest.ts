import type { ModuleManifest } from "@spitzli/module-sdk";
export const storageManifest = {
  id: "storage",
  version: "0.1.0",
  sdkVersion: 1,
  dependencies: [],
  navigation: [
    {
      slot: "tenant.services",
      id: "storage",
      label: "Storage",
      path: "/storage",
    },
  ],
} as const satisfies ModuleManifest;
