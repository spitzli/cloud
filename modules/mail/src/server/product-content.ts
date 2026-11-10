import type { MailProductStore } from "./provider-ports";
import type { DatabaseReader } from "@spitzli/core/identity";
import {
  createCipheriv,
  createDecipheriv,
  createHash,
  randomBytes,
} from "node:crypto";
import type { EmailRequest } from "@spitzli/mail-contracts";
type Dependencies = {
  contentSecret(): string | undefined;
  database: MailProductStore["database"];
};
type Context = { customerID: string; messageID: string };
export function createMailContent(dependencies: Dependencies) {
  const { database } = dependencies;
  function key(secret: string | undefined) {
    if (!secret || secret.length < 32)
      throw Error("Mail content encryption unavailable");
    return createHash("sha256")
      .update("webdock-mail-attachments-v1\0" + secret)
      .digest();
  }
  const aad = (context: Context, index: number) =>
    Buffer.from(`${context.customerID}:${context.messageID}:${index}`);
  function sealMailAttachments(
    context: Context,
    items: NonNullable<EmailRequest["attachments"]>,
    secret = dependencies.contentSecret(),
  ): string {
    if (
      items.length > 10 ||
      items.reduce((n, a) => n + Buffer.byteLength(a.content), 0) > 2_800_000
    )
      throw Error("Attachment content too large");
    const encryptionKey = key(secret);
    const result = JSON.stringify(
      items.map((item, index) => {
        const nonce = randomBytes(12),
          cipher = createCipheriv("aes-256-gcm", encryptionKey, nonce);
        cipher.setAAD(aad(context, index));
        const content = Buffer.concat([
          cipher.update(item.content, "utf8"),
          cipher.final(),
        ]);
        return [
          "v1",
          nonce.toString("base64url"),
          cipher.getAuthTag().toString("base64url"),
          content.toString("base64url"),
        ].join(".");
      }),
    );
    if (result.length > 4_000_000)
      throw Error("Attachment ciphertext too large");
    return result;
  }
  function openMailAttachments(
    context: Context,
    sealed: string,
    secret = dependencies.contentSecret(),
  ): string[] {
    try {
      if (sealed.length > 4_000_000) throw Error();
      const data: unknown = JSON.parse(sealed);
      if (!Array.isArray(data) || data.length > 10) throw Error();
      const encryptionKey = key(secret);
      return data.map((value, index) => {
        if (typeof value !== "string") throw Error();
        const parts = value.split(".");
        if (
          parts.length !== 4 ||
          parts[0] !== "v1" ||
          parts.slice(1).some((p) => !/^[A-Za-z0-9_-]*$/.test(p))
        )
          throw Error();
        const nonce = Buffer.from(parts[1], "base64url"),
          tag = Buffer.from(parts[2], "base64url");
        if (nonce.length !== 12 || tag.length !== 16) throw Error();
        const cipher = createDecipheriv("aes-256-gcm", encryptionKey, nonce);
        cipher.setAAD(aad(context, index));
        cipher.setAuthTag(tag);
        return Buffer.concat([
          cipher.update(Buffer.from(parts[3], "base64url")),
          cipher.final(),
        ]).toString("utf8");
      });
    } catch {
      throw Error("Stored Mail attachments cannot be opened");
    }
  }
  async function eraseAttachmentContent(messageID: string): Promise<void> {
    await database.query(
      "UPDATE webdock_auth.mail_message_content SET encrypted_attachments=NULL WHERE message_id=$1",
      [messageID],
    );
  }
  /** Expiry is monotonic: a late acceptance must not make an already hidden body readable again. */
  async function discardExpiredUncertainContent(
    db: DatabaseReader,
    messageID: string,
  ): Promise<void> {
    await db.query(
      "SELECT id FROM webdock_auth.mail_message WHERE id=$1 FOR UPDATE",
      [messageID],
    );
    await db.query(
      `DELETE FROM webdock_auth.mail_message_content c USING webdock_auth.mail_message m WHERE c.message_id=m.id AND m.id=$1
  AND (c.expires_at<=now() OR (m.dispatch_status IN ('dispatching','outcome_unknown') AND coalesce(m.handoff_started_at,m.created_at)+interval '7 days'<=now()))`,
      [messageID],
    );
  }
  return {
    sealMailAttachments,
    openMailAttachments,
    eraseAttachmentContent,
    discardExpiredUncertainContent,
  };
}
