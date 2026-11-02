/** User-credential data client. The broker supplies the account; callers cannot switch it. */
export class JmapError extends Error {
  readonly uncertain: boolean;
  constructor(message: string, uncertain = false) { super(message); this.uncertain = uncertain; }
}
export type MailAddress = { email: string; name?: string };
export type MailAttachment = { blobId: string; name: string; type: string; size: number };
export type MailDraft = { identityId: string; to: MailAddress[]; cc?: MailAddress[]; bcc?: MailAddress[]; subject: string; text: string; inReplyTo?: string[]; references?: string[]; attachments?: MailAttachment[] };
export const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;
function address(value: MailAddress): MailAddress {
  if (!value || typeof value.email !== "string" || value.email.length > 320 || !/^[^\s<>@,;]+@[^\s<>@,;]+\.[^\s<>@,;]+$/.test(value.email) ||
      value.name !== undefined && (typeof value.name !== "string" || value.name.length > 200 || /[\r\n\0]/.test(value.name))) throw new JmapError("Invalid Mail address.");
  return { email: value.email, ...(value.name ? { name: value.name } : {}) };
}
type Options = { url: string; authorization: string; accountId: string; sender?: MailAddress; allowInsecureHttp?: boolean; timeoutMs?: number };
export type MailObject = Record<string, unknown> & { id: string };
const object = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const id = (value: string) => {
  if (typeof value !== "string" || !/^[A-Za-z0-9_-]{1,255}$/.test(value)) throw new JmapError("Invalid Mail identifier.");
  return value;
};
const properties = ["id", "threadId", "mailboxIds", "keywords", "from", "to", "subject", "receivedAt", "preview", "hasAttachment", "size"];
export class JmapClient {
  private readonly endpoint: URL;
  private readonly authorization: string;
  private readonly timeoutMs: number;
  private readonly accountId: string;
  private readonly sender?: MailAddress;
  constructor(options: Options) {
    const url = new URL(options.url);
    if ((url.protocol !== "https:" && !(options.allowInsecureHttp && url.protocol === "http:")) ||
        url.username || url.password || url.hash || url.search || url.pathname !== "/") throw new JmapError("Invalid Mail endpoint.");
    if (!options.authorization || /[\r\n]/.test(options.authorization)) throw new JmapError("Invalid Mail credential.");
    this.endpoint = new URL("/jmap", url); this.authorization = options.authorization; this.accountId = id(options.accountId);
    this.sender = options.sender ? address(options.sender) : undefined;
    this.timeoutMs = options.timeoutMs ?? 10_000;
    if (!Number.isSafeInteger(this.timeoutMs) || this.timeoutMs < 1 || this.timeoutMs > 60_000) throw new JmapError("Invalid Mail timeout.");
  }
  private async call(method: string, args: Record<string, unknown>) {
    const mutation = method.endsWith("/set");
    const controller = new AbortController(), timer = setTimeout(() => controller.abort(), this.timeoutMs);
    let body: unknown;
    try {
      const response = await fetch(this.endpoint, { method: "POST", redirect: "error", signal: controller.signal,
        headers: { "Content-Type": "application/json", Authorization: this.authorization },
        body: JSON.stringify({ using: ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail", "urn:ietf:params:jmap:submission"], methodCalls: [[method, { ...args, accountId: this.accountId }, "webdock"]] }),
      });
      if (!response.ok || !response.body) { await response.body?.cancel(); throw new Error(); }
      const reader = response.body.getReader(), chunks: Uint8Array[] = []; let size = 0;
      try {
        for (;;) {
          const part = await reader.read(); if (part.done) break;
          size += part.value.byteLength; if (size > 4_194_304) throw new Error();
          chunks.push(part.value);
        }
        body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
    } catch { throw new JmapError(mutation ? "Mail change outcome is unknown. Do not repeat it." : "Mail request failed.", mutation); }
    finally { clearTimeout(timer); controller.abort(); }
    const responses = object(body) ? body.methodResponses : undefined;
    const response = Array.isArray(responses) && responses.length === 1 ? responses[0] : undefined;
    if (!Array.isArray(response) || response.length !== 3 || response[2] !== "webdock" || !object(response[1])) throw new JmapError("Invalid Mail response.", mutation);
    if (response[0] === "error" && response[1].type === "stateMismatch") throw new JmapError("Mailbox changed. Refresh before editing again.");
    if (response[0] === "error") throw new JmapError("Mail request rejected.", mutation && !["invalidArguments", "unknownMethod", "unknownCapability", "accountNotFound", "accountNotSupportedByMethod", "forbidden", "stateMismatch"].includes(String(response[1].type)));
    if (response[0] !== method) throw new JmapError("Invalid Mail response.", mutation);
    const result = response[1];
    if (result.accountId !== this.accountId) throw new JmapError("Mail response belongs to another account.", mutation);
    for (const key of ["notCreated", "notUpdated", "notDestroyed"]) {
      const errors = result[key];
      if (object(errors) && Object.keys(errors).length) throw new JmapError("Mail change rejected.", Boolean(object(result.created) && Object.keys(result.created).length || object(result.updated) && Object.keys(result.updated).length || Array.isArray(result.destroyed) && result.destroyed.length) || Object.values(errors).some(error => !object(error) || !["invalidProperties", "notFound", "forbidden", "overQuota", "tooLarge", "tooManyRecipients", "invalidEmail", "invalidRecipients"].includes(String(error.type))));
    }
    return result;
  }
  private list(result: Record<string, unknown>, expected?: string[]): MailObject[] {
    if (!Array.isArray(result.list) || result.list.length > 100 || result.list.some(row => !object(row) || typeof row.id !== "string" || expected && !expected.includes(row.id))) throw new JmapError("Invalid Mail response.");
    return result.list as MailObject[];
  }
  async mailboxes() {
    const result = await this.call("Mailbox/get", { ids: null, properties: ["id", "name", "role", "parentId", "sortOrder", "totalEmails", "unreadEmails", "myRights"] });
    return { list: this.list(result), state: result.state };
  }
  async identities() { return this.list(await this.call("Identity/get", { ids: null })); }
  async emails(input: { mailboxId?: string; text?: string; position?: number; limit?: number } = {}) {
    const limit = input.limit ?? 50, position = input.position ?? 0;
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100) throw new JmapError("Invalid Mail page limit.");
    if (!Number.isSafeInteger(position) || position < 0 || position > 1_000_000) throw new JmapError("Invalid Mail page position.");
    if (input.text !== undefined && (typeof input.text !== "string" || input.text.length > 500)) throw new JmapError("Invalid Mail search.");
    const filter = { ...(input.mailboxId ? { inMailbox: id(input.mailboxId) } : {}), ...(input.text ? { text: input.text } : {}) };
    const query = await this.call("Email/query", { filter, position, limit, calculateTotal: true, sort: [{ property: "receivedAt", isAscending: false }] });
    if (!Array.isArray(query.ids) || query.ids.length > limit || !query.ids.every(value => typeof value === "string" && id(value))) throw new JmapError("Invalid Mail response.");
    const ids = query.ids as string[];
    const list = ids.length ? this.list(await this.call("Email/get", { ids, properties }), ids) : [];
    return { list, total: query.total, queryState: query.queryState, position };
  }
  async email(emailId: string) {
    const result = await this.call("Email/get", { ids: [id(emailId)], properties: [...properties, "blobId", "cc", "bcc", "replyTo", "sentAt", "messageId", "inReplyTo", "references", "textBody", "htmlBody", "attachments", "bodyValues"], fetchAllBodyValues: true, maxBodyValueBytes: 262144 });
    const list = this.list(result, [emailId]);
    if (list.length !== 1) throw new JmapError("Message is unavailable.");
    return list[0];
  }
  async updateEmail(emailId: string, change: { seen?: boolean; flagged?: boolean; mailboxIds?: string[] }, ifInState?: string) {
    id(emailId);
    const patch: Record<string, unknown> = {};
    for (const [field, keyword] of [["seen", "$seen"], ["flagged", "$flagged"]] as const) {
      if (change[field] !== undefined) {
        if (typeof change[field] !== "boolean") throw new JmapError("Invalid Mail change.");
        patch[`keywords/${keyword}`] = change[field] ? true : null;
      }
    }
    if (change.mailboxIds !== undefined) {
      if (!Array.isArray(change.mailboxIds) || change.mailboxIds.length < 1 || change.mailboxIds.length > 16) throw new JmapError("Invalid Mail folders.");
      patch.mailboxIds = Object.fromEntries(change.mailboxIds.map(value => [id(value), true]));
    }
    if (!Object.keys(patch).length) throw new JmapError("No Mail change supplied.");
    const result = await this.call("Email/set", { ...(ifInState ? { ifInState } : {}), update: { [emailId]: patch } });
    if (!object(result.updated) || !Object.hasOwn(result.updated, emailId)) throw new JmapError("Invalid Mail change response.", true);
    return { state: result.newState };
  }
  private async identity(identityId: string): Promise<MailAddress> {
    const item = (await this.identities()).find(row => row.id === id(identityId));
    if (!item || typeof item.email !== "string") throw new JmapError("Mail identity is unavailable.");
    if (this.sender) {
      if (item.email !== this.sender.email) throw new JmapError("Mail identity is unavailable.");
      return this.sender;
    }
    return address({ email: item.email, ...(typeof item.name === "string" ? { name: item.name } : {}) });
  }
  async createDraft(input: MailDraft, previousId?: string) {
    if (typeof input.subject !== "string" || input.subject.length > 998 || /[\r\n\0]/.test(input.subject) ||
        typeof input.text !== "string" || Buffer.byteLength(input.text) > 262144) throw new JmapError("Invalid Mail draft.");
    const recipients = [input.to, input.cc ?? [], input.bcc ?? []];
    if (recipients.some(list => !Array.isArray(list)) || recipients.flat().length > 100) throw new JmapError("Invalid Mail recipients.");
    const [to, cc, bcc] = recipients.map(list => list.map(address));
    const attachments = input.attachments ?? [];
    if (!Array.isArray(attachments) || attachments.length > 10 || attachments.some(part => !part || typeof part.name !== "string" || !part.name || part.name.length > 200 || /[\r\n\0]/.test(part.name) || typeof part.type !== "string" || !/^[a-z0-9!#$&^_.+-]+\/[a-z0-9!#$&^_.+-]+$/i.test(part.type) || !Number.isSafeInteger(part.size) || part.size < 0 || !id(part.blobId)) || attachments.reduce((sum,part)=>sum+part.size,0)>MAX_ATTACHMENT_BYTES) throw new JmapError("Invalid Mail attachments.");
    for (const values of [input.inReplyTo, input.references]) if (values !== undefined && (!Array.isArray(values) || values.length > 50 || values.some(value => typeof value !== "string" || value.length > 998 || !/^[^<>\s]+@[^<>\s]+$/.test(value)))) throw new JmapError("Invalid Mail reply reference.");
    const from = await this.identity(input.identityId);
    let previousState: string | undefined;
    if(previousId){
      const result=await this.call("Email/get",{ids:[id(previousId)],properties:["id","keywords","from"]});
      const previous=this.list(result,[previousId])[0];
      if(!previous || !object(previous.keywords) || !previous.keywords.$draft || !Array.isArray(previous.from) || previous.from.length!==1 || !object(previous.from[0]) || previous.from[0].email!==from.email || typeof result.state!=="string")throw new JmapError("Draft changed in another session. Reopen it before editing.");
      previousState=result.state;
    }
    const drafts = (await this.mailboxes()).list.find(row => row.role === "drafts");
    if (!drafts) throw new JmapError("Draft folder is unavailable.");
    const result = await this.call("Email/set", { ...(previousId ? {ifInState:previousState} : {}), create: { draft: {
      mailboxIds: { [drafts.id]: true }, keywords: { $draft: true }, from: [from], to, cc, bcc,
      subject: input.subject, textBody: [{ partId: "body", type: "text/plain" }], bodyValues: { body: { value: input.text } },
      ...(attachments.length ? { attachments: attachments.map(part=>({ blobId: part.blobId, name: part.name, type: part.type, disposition: "attachment" })) } : {}),
      ...(input.inReplyTo ? { inReplyTo: input.inReplyTo } : {}), ...(input.references ? { references: input.references } : {}),
    } } });
    const created = object(result.created) ? result.created.draft : undefined;
    if (!object(created) || typeof created.id !== "string" || !/^[A-Za-z0-9_-]{1,255}$/.test(created.id)) throw new JmapError("Invalid Mail draft response.", true);
    if(previousId){
      // JMAP set can partially succeed: never destroy the old draft in the create call.
      try{
        if(typeof result.newState!=="string")throw new Error();
        const cleanup=await this.call("Email/set",{ifInState:result.newState,destroy:[previousId]});
        if(!Array.isArray(cleanup.destroyed) || !cleanup.destroyed.includes(previousId))throw new Error();
        result.newState=cleanup.newState;
      }catch{throw new JmapError("Draft replacement outcome is unknown. Check Drafts before editing.",true);}
    }
    return { id: created.id, state: result.newState };
  }
  /** Caller durably reserves submission BEFORE invoking this non-retrying mutation. */
  async submitDraft(emailId: string, identityId: string) {
    const sender = await this.identity(identityId);
    const draft = this.list(await this.call("Email/get", { ids: [id(emailId)], properties: ["id", "from", "keywords"] }), [emailId])[0];
    if (!draft || !object(draft.keywords) || draft.keywords.$draft !== true || !Array.isArray(draft.from) || draft.from.length !== 1 ||
        !object(draft.from[0]) || draft.from[0].email !== sender.email) throw new JmapError("Draft sender does not match the selected identity.");
    const result = await this.call("EmailSubmission/set", { create: { submission: { emailId, identityId } } });
    const created = object(result.created) ? result.created.submission : undefined;
    if (!object(created) || typeof created.id !== "string" || !/^[A-Za-z0-9_-]{1,255}$/.test(created.id)) throw new JmapError("Mail submission outcome is unknown. Do not repeat it.", true);
    return { id: created.id, state: result.newState };
  }

  /** Separate from submission: filing can be repaired without sending again. */
  async fileSent(emailId: string) {
    const sent = (await this.mailboxes()).list.find(row => row.role === "sent");
    if (!sent) throw new JmapError("Sent folder is unavailable.");
    const result = await this.call("Email/set", { update: { [id(emailId)]: { "keywords/$draft": null, mailboxIds: { [sent.id]: true } } } });
    if (!object(result.updated) || !Object.hasOwn(result.updated, emailId)) throw new JmapError("Mail filing outcome is unknown.", true);
    return { state: result.newState };
  }

  private async binary(path: string, init: RequestInit, maximum: number) {
    const controller = new AbortController(), timer = setTimeout(()=>controller.abort(), this.timeoutMs);
    try {
      const response = await fetch(new URL(path, this.endpoint), { ...init, redirect: "error", signal: controller.signal, headers: { ...init.headers, Authorization: this.authorization } });
      if (!response.ok || !response.body) { await response.body?.cancel(); throw new Error(); }
      const reader=response.body.getReader(), parts:Uint8Array[]=[];let size=0;
      try { for(;;){const part=await reader.read();if(part.done)break;size+=part.value.byteLength;if(size>maximum)throw new Error();parts.push(part.value);} }
      finally {await reader.cancel().catch(()=>{});reader.releaseLock();}
      return Buffer.concat(parts);
    }catch{throw new JmapError("Mail attachment transfer failed.");}finally{clearTimeout(timer);controller.abort();}
  }
  async upload(bytes: Buffer, type: string) {
    if (!Buffer.isBuffer(bytes) || bytes.length > MAX_ATTACHMENT_BYTES || type.length>127 || !/^[a-z0-9!#$&^_.+-]+\/[a-z0-9!#$&^_.+-]+$/i.test(type)) throw new JmapError("Invalid Mail attachment.");
    const raw=await this.binary(`/jmap/upload/${this.accountId}/`,{method:"POST",headers:{"Content-Type":type},body:new Uint8Array(bytes)},65536);
    let result:unknown;try{result=JSON.parse(raw.toString());}catch{throw new JmapError("Invalid Mail attachment response.");}
    if(!object(result) || result.accountId!==this.accountId || result.size!==bytes.length || typeof result.blobId!=="string" || typeof result.type!=="string")throw new JmapError("Invalid Mail attachment response.");
    return {blobId:id(result.blobId),size:bytes.length,type};
  }
  async exportDraft(emailId:string,identityId:string){
    const sender=await this.identity(identityId);
    const row=this.list(await this.call("Email/get",{ids:[id(emailId)],properties:["id","blobId","size","keywords","from","to","cc","bcc"]}),[emailId])[0];
    if(!row || !object(row.keywords) || row.keywords.$draft!==true || !Array.isArray(row.from) || row.from.length!==1 || !object(row.from[0]) || row.from[0].email!==sender.email || typeof row.blobId!=="string" || typeof row.size!=="number" || row.size>25*1024*1024)throw new JmapError("Draft sender does not match the selected identity.");
    const lists=[row.to??[],row.cc??[],row.bcc??[]];
    if(lists.some(list=>!Array.isArray(list)))throw new JmapError("Invalid Mail recipients.");
    const recipients=(lists as unknown[][]).flat().map(item=>{
      if(!object(item) || typeof item.email!=="string")throw new JmapError("Invalid Mail recipients.");
      return address({email:item.email}).email;
    });
    if(!recipients.length || recipients.length>100)throw new JmapError("Invalid Mail recipients.");
    const blobId=id(row.blobId);
    const raw=await this.binary(`/jmap/download/${this.accountId}/${blobId}/message.eml?accept=message%2Frfc822`,{method:"GET"},25*1024*1024);
    return {sender,to:recipients,raw,blobId};
  }
  async downloadAttachment(emailId: string, blobId: string) {
    id(blobId);
    const email=await this.email(emailId);
    const attachment=Array.isArray(email.attachments)?email.attachments.find(part=>object(part) && part.blobId===blobId):undefined;
    if(!object(attachment) || typeof attachment.name!=="string" || typeof attachment.size!=="number" || attachment.size>MAX_ATTACHMENT_BYTES)throw new JmapError("Mail attachment is unavailable or too large.");
    const bytes=await this.binary(`/jmap/download/${this.accountId}/${blobId}/attachment?accept=application%2Foctet-stream`,{method:"GET"},MAX_ATTACHMENT_BYTES);
    return {bytes,name:attachment.name};
  }

}
