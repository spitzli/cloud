import {
  createCipheriv,
  createDecipheriv,
  createHash,
  randomBytes,
} from "node:crypto";
import { isIP } from "node:net";
import { database, authDatabase } from "../../db/runtime";
import { currentMCPClaims } from "../auth/mcp";

export class PlatformError extends Error {}
export type PlatformSettings = {
  name: string;
  supportEmail: string;
  cmsEnabled: boolean;
  mailEnabled: boolean;
  mailGatewayEnabled: boolean;
  mailDispatchEnabled: boolean;
  mailWebhooksEnabled: boolean;
  mailSendingIP: string;
  mailDefaultLimit: number;
  mailRegion: "eu" | "global";
};
export const platformDefaults: PlatformSettings = {
  name: "Webdock",
  supportEmail: "dominik@spitzli.dev",
  cmsEnabled: true,
  mailEnabled: false,
  mailGatewayEnabled: false,
  mailDispatchEnabled: false,
  mailWebhooksEnabled: false,
  mailSendingIP: "",
  mailDefaultLimit: 1000,
  mailRegion: "eu",
};
export const platformSchemaSQL = `
CREATE TABLE IF NOT EXISTS webdock_auth.platform_settings (
 id boolean PRIMARY KEY DEFAULT true CHECK(id), settings jsonb NOT NULL DEFAULT '{}',
 updated_by text, updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS webdock_auth.platform_mail_credential (
 id boolean PRIMARY KEY DEFAULT true CHECK(id), encrypted_secret text NOT NULL,
 key_suffix text NOT NULL, updated_by text NOT NULL, updated_at timestamptz NOT NULL DEFAULT now()
);`;
export function validatePlatformSettings(
  input: Record<string, string>,
): PlatformSettings {
  const name = (input.name || "").trim(),
    supportEmail = (input.supportEmail || "").trim();
  const mailSendingIP = (input.mailSendingIP || "").trim();
  const limit = input.mailDefaultLimit || "";
  if (!name || name.length > 80 || /[\x00-\x1f\x7f]/.test(name))
    throw new PlatformError("Enter a platform name of up to 80 characters.");
  if (
    supportEmail.length > 254 ||
    !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(supportEmail)
  )
    throw new PlatformError("Enter a valid support email.");
  if (mailSendingIP && isIP(mailSendingIP) !== 4)
    throw new PlatformError(
      "Use the IPv4 sending address assigned by the mail provider.",
    );
  if (!/^\d{1,10}$/.test(limit) || Number(limit) > 1_000_000_000)
    throw new PlatformError(
      "Choose a default mail limit from 0 to 1,000,000,000.",
    );
  if (!["eu", "global"].includes(input.mailRegion))
    throw new PlatformError("Choose EU or global sending infrastructure.");
  return {
    name,
    supportEmail,
    mailSendingIP,
    cmsEnabled: input.cmsEnabled === "yes",
    mailEnabled: input.mailEnabled === "yes",
    mailGatewayEnabled: input.mailGatewayEnabled === "yes",
    mailDispatchEnabled: input.mailDispatchEnabled === "yes",
    mailWebhooksEnabled: input.mailWebhooksEnabled === "yes",
    mailDefaultLimit: Number(limit),
    mailRegion: input.mailRegion as "eu" | "global",
  };
}
export async function getPlatformSettings(): Promise<PlatformSettings> {
  const row = (
    await database.query(
      "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  return { ...platformDefaults, ...row?.settings };
}
// Public branding must not prevent access to the sign-in page during a database outage.
export async function publicPlatformSettings() {
  try {
    const row = (
      await authDatabase.query(
        "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
      )
    ).rows[0];
    const { name, supportEmail } = { ...platformDefaults, ...row?.settings };
    return { name, supportEmail };
  } catch {
    return {
      name: platformDefaults.name,
      supportEmail: platformDefaults.supportEmail,
    };
  }
}
async function requireRoot(actorID: string) {
  if ((await currentMCPClaims(actorID)).disabled)
    throw new PlatformError("Platform operator access is required.");
}
export function encryptMailSecret(
  value: unknown,
  purpose: string,
  secret = process.env.BETTER_AUTH_SECRET,
) {
  if (!secret || secret.length < 32)
    throw new PlatformError("Mail credential encryption is not configured.");
  const key = createHash("sha256")
    .update("webdock-mail-v1\0" + secret)
    .digest();
  const nonce = randomBytes(12),
    cipher = createCipheriv("aes-256-gcm", key, nonce);
  cipher.setAAD(Buffer.from(purpose));
  const ciphertext = Buffer.concat([
    cipher.update(JSON.stringify(value), "utf8"),
    cipher.final(),
  ]);
  return [
    "v1",
    nonce.toString("base64url"),
    cipher.getAuthTag().toString("base64url"),
    ciphertext.toString("base64url"),
  ].join(".");
}
export function decryptMailSecret<T>(
  value: string,
  purpose: string,
  secret = process.env.BETTER_AUTH_SECRET,
): T {
  try {
    if (!secret || secret.length < 32 || value.length > 32768) throw Error();
    const parts = value.split(".");
    if (
      parts.length !== 4 ||
      parts[0] !== "v1" ||
      parts.slice(1).some((p) => !/^[A-Za-z0-9_-]+$/.test(p))
    )
      throw Error();
    const nonce = Buffer.from(parts[1], "base64url"),
      tag = Buffer.from(parts[2], "base64url");
    if (nonce.length !== 12 || tag.length !== 16) throw Error();
    const decipher = createDecipheriv(
      "aes-256-gcm",
      createHash("sha256")
        .update("webdock-mail-v1\0" + secret)
        .digest(),
      nonce,
    );
    decipher.setAAD(Buffer.from(purpose));
    decipher.setAuthTag(tag);
    return JSON.parse(
      Buffer.concat([
        decipher.update(Buffer.from(parts[3], "base64url")),
        decipher.final(),
      ]).toString("utf8"),
    );
  } catch {
    throw new PlatformError(
      "Stored mail credentials could not be opened. Reconnect the provider.",
    );
  }
}
async function rootWrite(
  actorID: string,
  action: string,
  query: string,
  parameters: unknown[],
) {
  await requireRoot(actorID);
  const client = await database.connect();
  try {
    await client.query("BEGIN");
    await client.query(query, parameters);
    await client.query(
      "INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,$2,'platform','succeeded')",
      [actorID, action],
    );
    await client.query("COMMIT");
  } catch (error) {
    await client.query("ROLLBACK");
    throw error;
  } finally {
    client.release();
  }
}
export async function savePlatformSettings(
  actorID: string,
  input: Record<string, string>,
) {
  const settings = validatePlatformSettings(input);
  await rootWrite(
    actorID,
    "platform-settings",
    "INSERT INTO webdock_auth.platform_settings(id,settings,updated_by) VALUES(true,$1,$2) ON CONFLICT(id) DO UPDATE SET settings=$1,updated_by=$2,updated_at=now()",
    [JSON.stringify(settings), actorID],
  );
}
export async function getMailConnectionStatus(): Promise<{
  configured: boolean;
  consumerKeySuffix?: string;
}> {
  if (process.env.DATABASE_RLS_SECRET)
    return (
      await database.query(
        "SELECT webdock_rls.mail_connection_status() AS status",
      )
    ).rows[0].status;
  const row = (
    await database.query(
      "SELECT key_suffix FROM webdock_auth.platform_mail_credential WHERE id=true",
    )
  ).rows[0];
  return row
    ? { configured: true, consumerKeySuffix: row.key_suffix }
    : { configured: false };
}
export async function saveMailCredentials(
  actorID: string,
  consumerKey: string,
  consumerSecret: string,
) {
  if (
    [consumerKey, consumerSecret].some(
      (v) => !v || v.length > 4096 || /[\s\x00-\x1f\x7f]/.test(v),
    )
  )
    throw new PlatformError("Enter both provider keys without whitespace.");
  const sealed = encryptMailSecret(
    { consumerKey, consumerSecret },
    "platform-master",
  );
  await rootWrite(
    actorID,
    "mail-credentials-saved",
    "INSERT INTO webdock_auth.platform_mail_credential(id,encrypted_secret,key_suffix,updated_by) VALUES(true,$1,$2,$3) ON CONFLICT(id) DO UPDATE SET encrypted_secret=$1,key_suffix=$2,updated_by=$3,updated_at=now()",
    [sealed, consumerKey.slice(-4), actorID],
  );
}
export async function clearMailCredentials(actorID: string) {
  await rootWrite(
    actorID,
    "mail-credentials-removed",
    "DELETE FROM webdock_auth.platform_mail_credential WHERE id=true",
    [],
  );
}
