import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwind from "@tailwindcss/vite";

// Explicit test-only entry point. The normal Console build never imports this fixture.
export default defineConfig({
  root: fileURLToPath(new URL("../", import.meta.url)),
  cacheDir: fileURLToPath(new URL("../../../.runtime-test/vite-cache/neon-plugin-fixture", import.meta.url)),
  plugins: [react(), tailwind()],
  resolve: {
    alias: { "@spitzli/plugin-ui": fileURLToPath(new URL("../src/plugins/sdk.ts", import.meta.url)) },
    dedupe: ["react", "react-dom", "@tanstack/react-query", "zod"],
  },
  build: {
    target: "es2022",
    outDir: fileURLToPath(new URL("../../../.runtime-test/neon-plugin-fixture", import.meta.url)),
    emptyOutDir: true,
    rolldownOptions: { input: fileURLToPath(new URL("./neon-plugin-fixture.html", import.meta.url)) },
  },
});
