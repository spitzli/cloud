import path from "node:path";
import { fileURLToPath } from "node:url";
import { buildConfig, type AuthStrategy } from "payload";
import { postgresAdapter } from "@payloadcms/db-postgres";
import sharp from "sharp";
import { verifiedPostgresConfig } from "@webdock/database-runtime";
import { payloadEnvironment } from "./lib/config.ts";
import { pages, media, settings, users } from "./lib/content.ts";
import { sso } from "./lib/sso.ts";
import { mediaStorageAdapter } from "./lib/media-storage.ts";
const runtime = payloadEnvironment();
const directory = path.dirname(fileURLToPath(import.meta.url));
export default buildConfig({
  secret: runtime.secret,
  serverURL: runtime.origin,
  admin: { disable: true },
  collections: [
    users(
      sso
        ? [
            {
              name: "webdock-sso",
              authenticate: ({ headers }) => sso!.authenticate({ headers }),
            } as AuthStrategy,
          ]
        : [],
    ),
    pages,
    media(runtime.mediaDirectory),
  ],
  globals: [settings],
  storage: [mediaStorageAdapter(runtime)],
  db: postgresAdapter({
    schemaName: "app",
    disableCreateDatabase: true,
    push: false,
    pool: {
      ...verifiedPostgresConfig(runtime.databaseURL, runtime.databaseCA),
      max: 3,
    },
    migrationDir: path.join(directory, "migrations"),
  }),
  localization: { locales: ["en", "de"], defaultLocale: "en", fallback: false },
  upload: { limits: { fileSize: 10 * 1024 * 1024 }, abortOnLimit: true },
  graphQL: { disable: true },
  sharp,
  telemetry: false,
  typescript: { outputFile: path.join(directory, "payload-types.ts") },
});
