import { z } from "zod";
const recordID = z.string().regex(/^[1-9][0-9]{0,18}$/);
export const jmapID = z.string().regex(/^[A-Za-z0-9_-]{1,255}$/);
export const httpsOrigin = z.string().refine((value) => {
  try {
    const url = new URL(value);
    return url.protocol === "https:" && url.origin === value && !url.username && !url.password;
  } catch { return false; }
}, "Webmail requires an exact HTTPS origin.");
const expiresAt = z.iso.datetime({ offset: true });
export const launchSchema = z.object({ data: z.object({
  code: z.string().regex(/^[A-Za-z0-9_-]{43}$/), gatewayOrigin: httpsOrigin,
  customerID: recordID, mailboxID: recordID, expiresAt,
}) });
export type WebmailLaunch = z.infer<typeof launchSchema>;
export const exchangeSchema = z.object({ scopeID: recordID, customerID: recordID, mailboxID: recordID, expiresAt });
const text = z.string().max(262144);
const nullableText = text.nullish().transform((value) => value ?? "");
export const addressSchema = z.object({ email: z.string().max(320), name: z.string().max(200).nullish() });
const addresses = z.array(addressSchema).max(100).nullish().transform((value) => value ?? []);
export const attachmentSchema = z.object({
  blobId: jmapID, name: z.string().max(200), type: z.string().max(200), size: z.number().int().nonnegative().max(10485760),
});
export type MailAttachment = z.infer<typeof attachmentSchema>;
export const folderSchema = z.object({
  id: jmapID, name: z.string().max(1000), role: z.string().max(80).nullish(),
  unreadEmails: z.number().int().nonnegative().default(0),
});
export type MailFolder = z.infer<typeof folderSchema>;
const folderRoles = ["inbox", "drafts", "sent", "archive", "junk", "trash"];
export function orderFolders(folders: MailFolder[]): MailFolder[] {
  const rank = (folder: MailFolder) => { const index = folderRoles.indexOf(folder.role ?? ""); return index < 0 ? folderRoles.length : index; };
  return [...folders].sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
}
export const foldersSchema = z.object({ data: z.object({ list: z.array(folderSchema).max(1000) }) });
export const identitiesSchema = z.object({ data: z.array(z.object({
  id: jmapID, email: z.string().max(320), name: z.string().max(200).nullish(),
})).max(100) });
const bodyPart = z.object({ partId: z.string().max(255) });
export const emailSchema = z.object({
  id: jmapID, subject: nullableText, preview: nullableText,
  from: addresses, to: addresses, cc: addresses, bcc: addresses, replyTo: addresses,
  receivedAt: z.string().max(80).nullish(),
  keywords: z.record(z.string().max(100), z.boolean()).nullish().transform((value) => value ?? {}),
  messageId: z.array(z.string().max(998)).max(50).nullish(),
  references: z.array(z.string().max(998)).max(50).nullish(),
  inReplyTo: z.array(z.string().max(998)).max(50).nullish(),
  textBody: z.array(bodyPart).max(100).nullish().transform((value) => value ?? []),
  htmlBody: z.array(bodyPart).max(100).nullish().transform((value) => value ?? []),
  bodyValues: z.record(z.string().max(255), z.object({ value: text, isTruncated: z.boolean().optional() })).nullish().transform((value) => value ?? {}),
  attachments: z.array(attachmentSchema).max(100).nullish().transform((value) => value ?? []),
});
export type MailEmail = z.infer<typeof emailSchema>;
export const listSchema = z.object({ data: z.object({
  list: z.array(emailSchema).max(100), total: z.number().int().nonnegative(), position: z.number().int().nonnegative(),
}) });
export const messageSchema = z.object({ data: emailSchema });
export const savedSchema = z.object({ data: z.object({ id: jmapID }) });
export const updatedSchema = z.object({ data: z.object({ state: z.string().max(1000) }) });
export const uploadSchema = z.object({ data: attachmentSchema });
export const submissionValue = z.object({
  id: recordID,
  state: z.enum(["staging", "queued", "leased", "handoff", "accepted", "rejected", "cancelled", "unknown", "preparation_review"]),
  filed: z.boolean(), filingState: z.enum(["pending", "complete", "needs_review"]).optional(),
});
export type MailSubmission = z.infer<typeof submissionValue>;
export const submissionSchema = z.object({ data: submissionValue });
export const lookupSchema = z.object({ data: submissionValue.nullable() });
const inputAddress = z.object({ email: z.email().max(320), name: z.string().max(200).optional() });
export const draftSchema = z.strictObject({
  identityId: jmapID, to: z.array(inputAddress).max(100), cc: z.array(inputAddress).max(100), bcc: z.array(inputAddress).max(100),
  subject: z.string().max(998).refine((value) => !/[\0\r\n]/.test(value)),
  text: z.string().refine((value) => new TextEncoder().encode(value).length <= 262144),
  attachments: z.array(attachmentSchema).max(10),
  inReplyTo: z.array(z.string().max(998)).max(50).optional(),
  references: z.array(z.string().max(998)).max(50).optional(),
}).refine((value) => value.to.length + value.cc.length + value.bcc.length <= 100)
  .refine((value) => value.attachments.reduce((sum, item) => sum + item.size, 0) <= 10485760);
export type MailDraft = z.infer<typeof draftSchema>;
export const gatewayCommand = z.discriminatedUnion("action", [
  z.strictObject({ action: z.literal("folders") }),
  z.strictObject({ action: z.literal("identities") }),
  z.strictObject({ action: z.literal("list"), mailboxId: jmapID, text: z.string().max(500), position: z.number().int().min(0).max(1000000), limit: z.number().int().min(1).max(100) }),
  z.strictObject({ action: z.literal("read"), emailId: jmapID }),
  z.strictObject({ action: z.literal("update"), requestID: z.uuid(), emailId: jmapID, change: z.strictObject({
    seen: z.boolean().optional(), flagged: z.boolean().optional(), mailboxIds: z.array(jmapID).min(1).max(16).optional(),
  }) }),
  z.strictObject({ action: z.literal("draft"), requestID: z.uuid(), draft: draftSchema, previousId: jmapID.optional() }),
  z.strictObject({ action: z.literal("submit"), requestID: z.uuid(), emailId: jmapID, identityId: jmapID }),
  z.strictObject({ action: z.literal("submission-lookup"), emailId: jmapID }),
  z.strictObject({ action: z.literal("submission"), id: recordID, operation: z.enum(["status", "cancel", "file"]) }),
]);
export type GatewayCommand = z.infer<typeof gatewayCommand>;
export function messageBody(message: MailEmail, kind: "textBody" | "htmlBody"): string {
  return message[kind].map((part) => message.bodyValues[part.partId]?.value ?? "").join("\n\n");
}
export function displayAddresses(value: MailEmail["from"]): string {
  return value.map((item) => item.name ? `${item.name} <${item.email}>` : item.email).join(", ");
}
