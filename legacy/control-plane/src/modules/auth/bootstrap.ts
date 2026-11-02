// Offline provisioning helpers. Never expose these through an HTTP route.
import { hashPassword } from "better-auth/crypto";
import { offlineProvisioning } from "./offline";
import { auth } from "./auth";
import { database } from "../../db/runtime";
export async function createIdentity(input: {
  email: string;
  name: string;
  password: string;
  operator?: boolean;
  mustChangePassword?: boolean;
}) {
  const password = await hashPassword(input.password);
  const client = await database.connect();
  try {
    await client.query("BEGIN");
    const user = (
      await client.query(
        `INSERT INTO webdock_auth."user"(name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES($1,$2,true,$3,false,$4) RETURNING id,email,name,role`,
        [
          input.name,
          input.email,
          input.operator ? "operator" : "user",
          input.mustChangePassword ?? true,
        ],
      )
    ).rows[0];
    await client.query(
      `INSERT INTO webdock_auth.account("accountId","providerId","userId",password,"updatedAt") VALUES($1,'credential',$1,$2,now())`,
      [user.id, password],
    );
    await client.query("COMMIT");
    return user as { id: string; email: string; name: string; role: string };
  } catch (error) {
    await client.query("ROLLBACK");
    throw error;
  } finally {
    client.release();
  }
}
export async function registerApplication(input: {
  label: string;
  origin: string;
  logoutPath: string;
  organizationID?: string;
  headers: Headers;
}) {
  const target = new URL(input.origin);
  const local =
    process.env.NODE_ENV !== "production" &&
    target.protocol === "http:" &&
    ["localhost", "127.0.0.1"].includes(target.hostname);
  if (target.protocol !== "https:" && !local)
    throw Error(
      "Application must use HTTPS or explicit local development loopback",
    );
  const ids = (
    await database.query(
      "SELECT webdock_auth.next_snowflake() AS binding,webdock_auth.next_snowflake() AS client",
    )
  ).rows[0];
  const binding = ids.binding as string;
  const client = await offlineProvisioning.run({ clientID: ids.client }, () =>
    auth.api.adminCreateOAuthClient({
      headers: input.headers,
      body: {
        client_name: input.label,
        application_type: local ? "native" : "web",
        redirect_uris: [input.origin + "/api/sso/callback"],
        post_logout_redirect_uris: [input.origin + input.logoutPath],
        scope: "openid profile email",
        grant_types: ["authorization_code"],
        response_types: ["code"],
        token_endpoint_auth_method: "client_secret_post",
        require_pkce: true,
        skip_consent: true,
        enable_end_session: true,
        metadata: { webdock_binding: binding },
      },
    }),
  );
  await database.query(
    "INSERT INTO webdock_auth.app_binding(id,client_id,label,organization_id) VALUES($1,$2,$3,$4)",
    [binding, client.client_id, input.label, input.organizationID || null],
  );
  return {
    binding,
    clientID: client.client_id,
    clientSecret: client.client_secret!,
  };
}
