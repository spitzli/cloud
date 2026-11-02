import { readFileSync } from "node:fs";
import {
  backfillMailConnections,
  type ByokBackfillMapping,
} from "../../src/modules/mail/product/backfill";
import { database } from "../../src/db/runtime";
import { MailError } from "../../src/modules/mail/product/access";
const [mode, target, mappingFile] = process.argv.slice(2);
try {
  const url = new URL(process.env.DATABASE_URL!);
  if (
    !["--dry-run", "--apply"].includes(mode) ||
    target !== `${url.hostname}:${url.port || "5432"}${url.pathname}`
  )
    throw Error("target");
  const byok: ByokBackfillMapping[] = mappingFile
    ? JSON.parse(readFileSync(mappingFile, "utf8"))
    : [];
  if (!Array.isArray(byok)) throw Error("mapping");
  console.log(
    JSON.stringify(
      await backfillMailConnections({ dryRun: mode === "--dry-run", byok }),
    ),
  );
} catch (error) {
  console.error(
    JSON.stringify({
      error: error instanceof MailError ? error.code : "backfill_failed",
      usage:
        "mail-backfill.ts --dry-run|--apply HOST:PORT/DATABASE [BYOK_MAPPING.json]",
    }),
  );
  process.exitCode = 1;
} finally {
  await database.end();
}
