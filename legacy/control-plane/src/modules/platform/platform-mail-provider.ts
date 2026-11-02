import type { Fetch } from "../shared/http";
import { database } from "../../db/runtime";
import { PlatformError, decryptMailSecret } from "./platform-settings";
/** Server-side only; never pass this result to a React client or action response. */
export async function mailProvider(
  fetcher?: Fetch,
  customerID?: string,
  connectionID?: string,
) {
  const row = process.env.DATABASE_RLS_SECRET
    ? (
        await database.query(
          "SELECT webdock_rls.mail_provider_secret($1,$2) AS encrypted_secret",
          [customerID ?? null, connectionID ?? null],
        )
      ).rows[0]
    : (
        await database.query(
          "SELECT encrypted_secret FROM webdock_auth.platform_mail_credential WHERE id=true",
        )
      ).rows[0];
  if (!row?.encrypted_secret)
    throw new PlatformError(
      "Connect the mail provider in Webdock administration first.",
    );
  const credentials = decryptMailSecret<{
    consumerKey: string;
    consumerSecret: string;
  }>(row.encrypted_secret, "platform-master");
  if (!credentials.consumerKey || !credentials.consumerSecret)
    throw new PlatformError("Reconnect the mail provider.");
  const { TurboSMTPClient } = await import("../mail/turbosmtp");
  return new TurboSMTPClient(credentials, fetcher);
}
export async function testMailConnection() {
  const result = await (await mailProvider()).listSubaccounts({ limit: 1 });
  return { count: result.count };
}
