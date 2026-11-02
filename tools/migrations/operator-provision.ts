import { createHash, randomBytes } from "node:crypto";
import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import type { PoolClient } from "pg";
import {
  OperatorSetupError,
  type InitialOperatorInput,
} from "./operator-config";
const apiRequire = createRequire(
  new URL("../../legacy/control-plane/package.json", import.meta.url),
);
/** Matches oauth-provider 1.7.7 with JWT enabled and default hashed client-secret storage. */
export const oauthSecretHash = (secret: string) =>
  createHash("sha256").update(secret, "utf8").digest("base64url");
export async function initialPasswordHash(password: string) {
  const provider = JSON.parse(
    await readFile(
      join(
        dirname(apiRequire.resolve("@better-auth/oauth-provider")),
        "../package.json",
      ),
      "utf8",
    ),
  );
  if (provider.version !== "1.7.7")
    throw new OperatorSetupError(
      "Review the initial OAuth client contract for this SDK version before provisioning",
    );
  const crypto = (await import(apiRequire.resolve("better-auth/crypto"))) as {
    hashPassword: (value: string) => Promise<string>;
  };
  return crypto.hashPassword(password);
}
export function initialClient(input: InitialOperatorInput) {
  const specification = {
    version: 1,
    email: input.email,
    name: input.name,
    authOrigin: input.authOrigin,
    studioOrigin: input.studioOrigin,
    consoleOrigin: input.consoleOrigin,
    redirectURI: input.studioOrigin + "/api/sso/callback",
    logoutURI: input.studioOrigin + "/login",
    scopes: ["openid", "profile", "email"],
    requirePKCE: true,
    tokenEndpointAuthMethod: "client_secret_post",
  };
  return {
    specification,
    checksum: createHash("sha256")
      .update(JSON.stringify(specification))
      .digest("hex"),
  };
}
export type InitialOperatorSecrets = {
  operatorID: string;
  portalUserID: string;
  bindingID: string;
  clientID: string;
  checksum: string;
  environment: Record<string, string>;
};
/** Caller owns BEGIN/COMMIT and private output durability. Never creates a session or MFA material. */
export async function provisionInitialOperator(
  db: PoolClient,
  input: InitialOperatorInput,
): Promise<InitialOperatorSecrets> {
  await db.query(
    "SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='30s'",
  );
  await db.query(
    "SELECT pg_advisory_xact_lock(hashtext('webdock-api-migration'))",
  );
  const identity = (
    await db.query(
      "SELECT session_user AS login,current_user AS active,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=session_user",
    )
  ).rows[0];
  if (
    identity?.login !== "webdock_offline_migrate" ||
    identity.active !== "webdock_offline_migrate" ||
    identity.rolsuper ||
    identity.rolbypassrls
  )
    throw new OperatorSetupError(
      "A dedicated non-superuser offline migrator connection is required",
    );
  await db.query("SET LOCAL ROLE webdock_platform_owner");
  if (
    (
      await db.query(
        "SELECT checksum FROM webdock_api_migrations.baseline WHERE id",
      )
    ).rows[0]?.checksum !==
    (
      await readFile(new URL("./baseline.sha256", import.meta.url), "utf8")
    ).trim()
  )
    throw new OperatorSetupError(
      "Install the database baseline before initial operator setup",
    );
  await db.query(
    'LOCK TABLE webdock_auth."user",webdock_auth.account,webdock_auth."oauthClient",webdock_auth.app_binding,webdock_admin.users IN SHARE ROW EXCLUSIVE MODE',
  );
  if (
    (
      await db.query(
        `SELECT 1 FROM webdock_auth."user" WHERE role='operator' UNION ALL SELECT 1 FROM webdock_admin.users WHERE role='operator' UNION ALL SELECT 1 FROM webdock_auth.access_event WHERE action='operator.bootstrap' LIMIT 1`,
      )
    ).rowCount
  )
    throw new OperatorSetupError(
      "Initial operator setup already exists; use the authenticated account-management workflow",
    );
  if (
    (
      await db.query(
        'SELECT 1 FROM webdock_auth."user" WHERE lower(email)=lower($1) UNION ALL SELECT 1 FROM webdock_admin.users WHERE lower(email)=lower($1) LIMIT 1',
        [input.email],
      )
    ).rowCount
  )
    throw new OperatorSetupError(
      "An identity with this email already exists; automatic linking is forbidden",
    );
  if (
    (
      await db.query(
        'SELECT 1 FROM webdock_auth."oauthClient" UNION ALL SELECT 1 FROM webdock_auth.app_binding LIMIT 1',
      )
    ).rowCount
  )
    throw new OperatorSetupError(
      "OAuth applications already exist; initial setup never rebinds existing clients",
    );
  const password = await initialPasswordHash(input.password),
    secret = randomBytes(48).toString("base64url");
  const user = (
    await db.query(
      `INSERT INTO webdock_auth."user"(name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES($1,$2,true,'operator',false,true) RETURNING id`,
      [input.name, input.email],
    )
  ).rows[0];
  await db.query(
    `INSERT INTO webdock_auth.account("accountId","providerId","userId",password,"updatedAt") VALUES($1,'credential',$1,$2,now())`,
    [user.id, password],
  );
  const ids = (
    await db.query(
      "SELECT webdock_auth.next_snowflake() AS client,webdock_auth.next_snowflake() AS binding,webdock_admin.next_snowflake() AS portal",
    )
  ).rows[0];
  await db.query(
    `INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,$2,$3,'operator',$4)`,
    [ids.portal, input.name, input.email, user.id],
  );
  const { specification, checksum } = initialClient(input);
  // Metadata remains an object in PostgreSQL; auth's Drizzle adapter presents the JSON contract.
  const metadata = {
    webdock_binding: ids.binding,
    webdock_initial_setup: {
      ...specification,
      checksum,
      operatorID: user.id,
    },
  };
  await db.query(
    `INSERT INTO webdock_auth."oauthClient"("clientId","clientSecret",disabled,"skipConsent","enableEndSession",scopes,"clientCredentialsScopes","userId","createdAt","updatedAt",name,"redirectUris","postLogoutRedirectUris","tokenEndpointAuthMethod","applicationType","grantTypes","responseTypes","requirePKCE",metadata)
 VALUES($1,$2,false,true,true,$3,'[]'::jsonb,$4,now(),now(),'Webdock Console',$5,$6,'client_secret_post',$7,'["authorization_code"]'::jsonb,'["code"]'::jsonb,true,$8)`,
    [
      ids.client,
      oauthSecretHash(secret),
      JSON.stringify(specification.scopes),
      user.id,
      JSON.stringify([specification.redirectURI]),
      JSON.stringify([specification.logoutURI]),
      input.studioOrigin.startsWith("http:") ? "native" : "web",
      JSON.stringify(metadata),
    ],
  );
  await db.query(
    "INSERT INTO webdock_auth.app_binding(id,client_id,label,organization_id) VALUES($1,$2,'Webdock Console',NULL)",
    [ids.binding, ids.client],
  );
  // Fresh installations belong to their operator, never the upstream operator's address.
  await db.query(
    `INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,jsonb_build_object('supportEmail',$1::text,'cmsEnabled',false))
     ON CONFLICT(id) DO UPDATE SET settings=EXCLUDED.settings || webdock_auth.platform_settings.settings`,
    [input.email],
  );
  await db.query(
    "INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,'operator.bootstrap',$1,'succeeded'),($1,'console.bootstrap',$2,'succeeded')",
    [user.id, ids.client],
  );
  return {
    operatorID: user.id,
    portalUserID: ids.portal,
    bindingID: ids.binding,
    clientID: ids.client,
    checksum,
    environment: {
      BETTER_AUTH_URL: input.authOrigin,
      WEBDOCK_AUTH_ISSUER: input.authOrigin + "/api/auth",
      WEBDOCK_STUDIO_ORIGIN: input.studioOrigin,
      WEBDOCK_CONSOLE_ORIGIN: input.consoleOrigin,
      WEBDOCK_SSO_APP_ORIGIN: input.studioOrigin,
      WEBDOCK_SSO_CLIENT_ID: ids.client,
      WEBDOCK_STUDIO_CLIENT_ID: ids.client,
      WEBDOCK_SSO_CLIENT_SECRET: secret,
      WEBDOCK_SSO_COOKIE_SECRET: randomBytes(48).toString("base64url"),
    },
  };
}
