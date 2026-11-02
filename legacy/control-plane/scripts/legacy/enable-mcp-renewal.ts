// One-time policy update: Better Auth's insert-only seeding preserves old rows.
import { database } from "../../src/db/runtime";
import { mcpResource, mcpScopes } from "../../src/modules/auth/mcp";
const client = await database.connect();
try {
  await client.query("BEGIN");
  const { rows } = await client.query(
    'SELECT "allowedScopes",disabled FROM webdock_auth."oauthResource" WHERE identifier=$1 FOR UPDATE',
    [mcpResource],
  );
  const row = rows[0];
  if (
    !row ||
    row.disabled ||
    !Array.isArray(row.allowedScopes) ||
    !mcpScopes.every((scope) => row.allowedScopes.includes(scope))
  )
    throw Error(
      "Expected enabled Webdock MCP resource with read/write scopes; inspect its policy before updating.",
    );
  if (!row.allowedScopes.includes("offline_access")) {
    await client.query(
      'UPDATE webdock_auth."oauthResource" SET "allowedScopes"=$2::jsonb,"policyVersion"="policyVersion"+1,"updatedAt"=now() WHERE identifier=$1',
      [mcpResource, JSON.stringify([...row.allowedScopes, "offline_access"])],
    );
  }
  await client.query("COMMIT");
  console.log(
    "MCP resource allows opted-in token renewal; existing client permissions are unchanged.",
  );
} catch (error) {
  await client.query("ROLLBACK");
  throw error;
} finally {
  client.release();
  await database.end();
}
