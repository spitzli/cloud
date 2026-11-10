/** Explicit native Clerk setup for the disposable local browser fixture only. */
import { randomBytes } from "node:crypto";
import { lstat, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { parseEnv } from "node:util";
import type { Pool } from "pg";
import { readAuthProviderConfiguration } from "../../packages/auth-configuration/src/index";
const issuer = "https://on-sheep-3817.clerk.accounts.dev";
const userID = "user_3KqA27SPwkrKlDPfau3Rp8LwR0q";
async function input(path: string, privateFile = false) {
  const stat = await lstat(path);
  if (
    !stat.isFile() ||
    stat.isSymbolicLink() ||
    (privateFile && stat.mode & 0o077)
  )
    throw Error(
      "Clerk fixture requires regular input files and private credentials",
    );
  return readFile(path, "utf8");
}
export async function prepareClerkBrowser({
  dir,
  owner,
  secrets,
  env,
  credentialsPath,
  developmentEnvPath,
}: {
  dir: string;
  owner: Pick<Pool, "query">;
  secrets: { operatorID: string };
  env: Record<string, string | undefined>;
  credentialsPath: string;
  developmentEnvPath: string;
}) {
  if (
    env.NODE_ENV !== "development" ||
    env.SPITZLI_CONSOLE_ORIGIN !== "http://localhost:3460"
  )
    throw Error(
      "Native Clerk fixture requires the owned development Console origin",
    );
  const db = new URL(env.DATABASE_URL!);
  if (db.hostname !== "127.0.0.1" || db.pathname !== "/webdock_operator_test")
    throw Error(
      "Native Clerk fixture requires the disposable local operator database",
    );
  const credentials = JSON.parse(await input(credentialsPath, true));
  if (
    credentials.userID !== userID ||
    credentials.issuer !== issuer ||
    credentials.appID !== "app_3KjGKUGb7dYUMgkjKC0eCWghnOB" ||
    credentials.instanceID !== "ins_3KjGKQnAATh2cA4ddJBR2cg3NXu" ||
    !/^webdock-core-acceptance-/.test(credentials.externalID) ||
    typeof credentials.email !== "string" ||
    !credentials.email.endsWith("+clerk_test@example.com") ||
    typeof credentials.password !== "string" ||
    credentials.password.length < 15
  )
    throw Error(
      "Only the explicitly owned synthetic Clerk development user is allowed",
    );
  const keys = parseEnv(await input(developmentEnvPath));
  const secretKey = keys.CLERK_SECRET_KEY;
  const publishableKey =
    keys.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY ?? keys.CLERK_PUBLISHABLE_KEY;
  if (
    !secretKey?.startsWith("sk_test_") ||
    !publishableKey?.startsWith("pk_test_") ||
    Buffer.from(publishableKey.slice(8), "base64")
      .toString()
      .replace(/\$$/, "") !== new URL(issuer).hostname
  )
    throw Error("Clerk fixture requires matching dedicated development keys");
  const config = readAuthProviderConfiguration(
    JSON.stringify({
      provider: "clerk",
      issuer,
      authorizedParties: [env.SPITZLI_CONSOLE_ORIGIN],
      tokenMode: "session",
      allowLocalHTTP: true,
    }),
    env.NODE_ENV,
  );
  const actual = await owner.query("SELECT current_database() AS name");
  if (actual.rows[0]?.name !== "webdock_operator_test")
    throw Error("Unexpected fixture database");
  const mapped = await owner.query(
    `INSERT INTO webdock_auth.clerk_identity(issuer,clerk_user_id,user_id,legacy_eligible)
    SELECT $1,$2,id,false FROM webdock_auth."user" WHERE id=$3 AND role='operator' RETURNING user_id`,
    [issuer, userID, secrets.operatorID],
  );
  if (mapped.rows.length !== 1)
    throw Error("Seeded fixture operator required for explicit Clerk mapping");
  Object.assign(env, {
    SPITZLI_AUTH_PROVIDER: JSON.stringify(config),
    CLERK_SECRET_KEY: secretKey,
    CLERK_PUBLISHABLE_KEY: publishableKey,
    NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: publishableKey,
    SPITZLI_CONSOLE_SERVICE_KEY: randomBytes(48).toString("hex"),
    SPITZLI_OAUTH_PROVIDER: "disabled",
    SPITZLI_API_DOMAINS: "auth,portal,registry",
  });
  delete env.BETTER_AUTH_SECRET;
  const metadata = {
    synthetic: true,
    issuer,
    userID,
    localOperatorID: secrets.operatorID,
    consoleOrigin: env.SPITZLI_CONSOLE_ORIGIN,
    tokenMode: "session",
    oauthProvider: "disabled",
    legacySecretPresent: false,
    remoteMutation: false,
  };
  await writeFile(join(dir, "clerk-manifest.json"), JSON.stringify(metadata), {
    mode: 0o600,
  });
  return {
    email: credentials.email as string,
    password: credentials.password as string,
    clerk: metadata,
  };
}
