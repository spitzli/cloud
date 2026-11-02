import { draftSchema, displayAddresses, messageBody, type MailAttachment, type MailDraft, type MailEmail } from "./webmail-contracts";
export interface MailEditor {
  identityId: string; to: string; cc: string; bcc: string; subject: string; text: string;
  attachments: MailAttachment[]; previousId?: string; savedSignature?: string;
  inReplyTo?: string[]; references?: string[];
}
export function blankEditor(identityId: string): MailEditor {
  return { identityId, to: "", cc: "", bcc: "", subject: "", text: "", attachments: [] };
}
export function editorPayload(editor: MailEditor): MailDraft {
  const addresses = (value: string) => value.split(/[,;]/).map((email) => ({ email: email.trim() })).filter((value) => value.email);
  return draftSchema.parse({
    identityId: editor.identityId, to: addresses(editor.to), cc: addresses(editor.cc), bcc: addresses(editor.bcc),
    subject: editor.subject, text: editor.text, attachments: editor.attachments,
    ...(editor.inReplyTo ? { inReplyTo: editor.inReplyTo } : {}),
    ...(editor.references ? { references: editor.references } : {}),
  });
}
export function replyEditor(email: MailEmail, identityId: string): MailEditor {
  return {
    ...blankEditor(identityId), to: (email.replyTo.length ? email.replyTo : email.from).map((address) => address.email).join(", "),
    subject: email.subject.startsWith("Re:") ? email.subject : `Re: ${email.subject}`,
    text: `\n\n${displayAddresses(email.from)} wrote:\n${messageBody(email, "textBody").split("\n").map((line) => `> ${line}`).join("\n")}`,
    ...(email.messageId?.length ? { inReplyTo: email.messageId, references: [...(email.references ?? []), ...email.messageId].slice(-50) } : {}),
  };
}
