import { defineConfig } from "@neon/config/v1";

export default defineConfig({
  // Managed first factor; Core retains MFA step-up, canonical IDs and application authorization.
  auth: true,
  preview: {
    realtime: { allowedOrigins: ["https://console.spitzli.cloud", "https://studio.webdock.dev"] },
  },
  // Branch policy: per-branch tuning
  branch: (branch) => {
    if (branch.isDefault) {
      // Default branch: no overrides, uses project defaults
      return {};
    }
    if (!branch.exists) {
      // New non-default branches: auto-expire
      // Run `neon checkout <name>` to create a new branch with these settings
      return { ttl: "7d" };
    }
    // Existing branch: no changes
    return {};
  },
});
