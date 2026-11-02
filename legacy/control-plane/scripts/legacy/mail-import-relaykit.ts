import { readFileSync } from "node:fs";
import { importRelaykit } from "../../src/modules/mail/product/import";
import { MailError } from "../../src/modules/mail/product/access";
import { database } from "../../src/db/runtime";
const [mode, target, mappingFile] = process.argv.slice(2);
try {
  const url = new URL(process.env.DATABASE_URL!);
  if (
    !["--dry-run", "--apply"].includes(mode) ||
    target !== `${url.hostname}:${url.port || "5432"}${url.pathname}` ||
    !mappingFile ||
    !process.env.RELAYKIT_READONLY_DATABASE_URL
  )
    throw Error("arguments");
  const mapping = JSON.parse(readFileSync(mappingFile, "utf8"));
  console.log(
    JSON.stringify(
      await importRelaykit({
        sourceReadOnlyURL: process.env.RELAYKIT_READONLY_DATABASE_URL,
        mapping,
        dryRun: mode === "--dry-run",
      }),
    ),
  );
} catch (error) {
  console.error(
    JSON.stringify({
      error: error instanceof MailError ? error.code : "relaykit_import_failed",
      usage:
        "mail-import-relaykit.ts --dry-run|--apply TARGET_HOST:PORT/DATABASE MAPPING.json; source URL via RELAYKIT_READONLY_DATABASE_URL",
    }),
  );
  process.exitCode = 1;
} finally {
  await database.end();
}
