import vinext from "vinext";
import { defineConfig } from "vite";
import { nitro } from "nitro/vite";
import tailwindcss from "@tailwindcss/vite";
import nextConfig from "./next.config.ts";
import { patchGlobalRequest } from "srvx/node";

export default defineConfig(({ command }) => {
  // The RSC dev adapter uses lazy srvx requests; unwrap them before Bun clones a POST.
  if (command === "serve") patchGlobalRequest();
  return {
    optimizeDeps: {
      exclude: ["@payloadcms/next", "@payloadcms/ui"],
      include: [
        "@dnd-kit/core", "@dnd-kit/modifiers", "@dnd-kit/sortable",
        "ajv", "bson-objectid", "deepmerge", "escape-html", "fast-deep-equal",
        "hoist-non-react-statics", "http-status", "md5", "object-assign", "object-to-formdata",
        "pluralize", "prop-types", "react/compiler-runtime", "react-datepicker", "react-image-crop",
        "react-is", "react-select", "react-transition-group",
        "sanitize-filename", "scheduler", "use-sync-external-store/shim",
        "use-sync-external-store/shim/with-selector", "use-sync-external-store/with-selector",
      ],
    },
    ssr: { external: nextConfig.serverExternalPackages, noExternal: true },
    environments: { rsc: { resolve: { external: nextConfig.serverExternalPackages, noExternal: true } } },
    plugins: [tailwindcss(), vinext(), ...(command === "build" ? nitro({
      preset: process.env.NITRO_PRESET || "bun",
      noExternals: ["@vercel/og"],
      traceDeps: ["payload", "@payloadcms/db-postgres", "@payloadcms/email-nodemailer"],
      vercel: { functions: { regions: ["fra1"], runtime: "bun1.4.x" } },
    }) : [])],
  };
});
