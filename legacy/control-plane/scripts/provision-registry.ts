import { open } from "node:fs/promises";
import { basename } from "node:path";
import { randomBytes } from "node:crypto";
import { offlineOwner } from "./offline-owner";
const output = process.argv[4];
if (
  !output ||
  !basename(output).startsWith(".env.") ||
  /\.(example|sample|template)$/.test(output)
)
  throw Error(
    "Provide a new ignored .env.* output file for the registry credential",
  );
const { pool, url } = await offlineOwner(process.argv[2], process.argv[3]);
try {
  if (
    (
      await pool.query(
        "SELECT 1 FROM pg_roles WHERE rolname='webdock_registry_runtime'",
      )
    ).rowCount
  )
    throw Error(
      "Registry role exists. Recover its credential; do not silently rotate it.",
    );
  const file = await open(output, "wx", 0o600),
    password = randomBytes(40).toString("base64url");
  try {
    await pool.query("BEGIN");
    await pool.query(
      `CREATE ROLE webdock_registry_runtime LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS PASSWORD '${password}'`,
    );
    url.username = "webdock_registry_runtime";
    url.password = password;
    await file.writeFile(
      "# Prepared credential: verify role creation and install RLS before deployment.\nREGISTRY_DATABASE_URL=" +
        url.href +
        "\nREGISTRY_POOL_MAX=1\n",
    );
    await file.sync();
    await pool.query("COMMIT");
    console.log(
      "Registry principal created; credential written only to the private output file. Install the reviewed RLS policies before use.",
    );
  } catch {
    await pool.query("ROLLBACK").catch(() => {});
    throw Error(
      "Role provisioning was not confirmed. Preserve the private output file and inspect the role before retrying.",
    );
  } finally {
    await file.close();
  }
} finally {
  await pool.end();
}
