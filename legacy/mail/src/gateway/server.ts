import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { createHash, randomBytes } from "node:crypto";
import { JmapClient, JmapError, MAX_ATTACHMENT_BYTES, type MailDraft, type MailAttachment } from "../stalwart/jmap.ts";

type RecordValue = Record<string, unknown>;
type Options = { allowedOrigins: string[]; control: (body: RecordValue) => Promise<unknown>; testRuntimeOrigin?: string; testDirectSubmission?: boolean; nativeSending?: (body:RecordValue)=>Promise<unknown> };
type Session = { token: string; scopeID: string; customerID: string; mailboxID: string; expires: number; active: number; uploadedBytes: number; attachments: Map<string,MailAttachment> };
class GatewayError extends Error {
  readonly status: number;
  readonly uncertain: boolean;
  constructor(status: number, message: string, uncertain = false) { super(message); this.status = status; this.uncertain = uncertain; }
}
function record(value: unknown): RecordValue {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new GatewayError(400, "Invalid Mail request.");
  return value as RecordValue;
}
function fields(body: RecordValue, names: string[]) {
  if (Object.keys(body).some(key => !names.includes(key))) throw new GatewayError(400, "Invalid Mail request.");
}
function string(value: unknown, maximum = 255): string {
  if (typeof value !== "string" || !value || value.length > maximum) throw new GatewayError(400, "Invalid Mail request.");
  return value;
}
function identifier(value: unknown) {
  const text = string(value, 19); if (!/^[1-9][0-9]{0,18}$/.test(text)) throw new GatewayError(400, "Invalid Mail scope."); return text;
}
function nativeReceipt(value:unknown){
  const row=record(value),state=string(row.state,32);
  if(!['staging','queued','leased','handoff','accepted','rejected','cancelled','unknown','preparation_review'].includes(state))throw new GatewayError(502,'Invalid Mail submission receipt.',true);
  return {id:identifier(row.id),state,filed:row.filed===true,...(typeof row.filingState==='string' && ['pending','complete','needs_review'].includes(row.filingState)?{filingState:row.filingState}:{})};
}
function expires(value: unknown) {
  const time = Date.parse(string(value, 64));
  if (!Number.isFinite(time) || time <= Date.now() || time > Date.now()+16*60_000) throw new GatewayError(401, "Mail session expired.");
  return time;
}
async function read(request: IncomingMessage) {
  if (request.headers["content-type"]?.split(";")[0].trim() !== "application/json") throw new GatewayError(415, "Use application/json.");
  const parts: Buffer[] = []; let bytes = 0;
  for await (const part of request) {
    bytes += part.length; if (bytes > 524288) throw new GatewayError(413, "Mail request is too large."); parts.push(Buffer.from(part));
  }
  try { return record(JSON.parse(Buffer.concat(parts).toString())); }
  catch (error) { if (error instanceof GatewayError) throw error; throw new GatewayError(400, "Invalid Mail request."); }
}
const json = (response: ServerResponse, status: number, value: unknown) => {
  response.writeHead(status, { "Content-Type": "application/json", "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff" });
  response.end(JSON.stringify(value));
};

export function createMailGateway(options: Options) {
  const origins = new Set(options.allowedOrigins.map(value => {
    const url = new URL(value); if (url.protocol !== "https:" || url.origin !== value) throw Error("Mail gateway requires exact HTTPS origins"); return value;
  }));
  if (!origins.size || options.testDirectSubmission && process.env.NODE_ENV === "production" || options.testRuntimeOrigin && (process.env.NODE_ENV === "production" || !/^http:\/\/127\.0\.0\.1:[0-9]+$/.test(options.testRuntimeOrigin))) throw Error("Invalid Mail gateway configuration");
  const sessions = new Map<string, Session>(); let active = 0;
  const server = createServer(async (request, response) => {
    let running: Session | undefined, admitted = false;
    try {
      if (request.method === "GET" && request.url === "/healthz") { json(response, 200, { ok: true }); return; }
      if (!request.headers.origin || !origins.has(request.headers.origin)) throw new GatewayError(403, "Invalid origin.");
      response.setHeader("Access-Control-Allow-Origin", request.headers.origin);
      response.setHeader("Access-Control-Allow-Credentials", "true"); response.setHeader("Vary", "Origin");
      if (request.method === "OPTIONS") {
        response.setHeader("Access-Control-Allow-Methods", "POST"); response.setHeader("Access-Control-Allow-Headers", "Content-Type, X-Webdock-Filename"); response.writeHead(204); response.end(); return;
      }
      if (request.method !== "POST") throw new GatewayError(405, "Use POST.");
      if (active >= 16) throw new GatewayError(429, "Mail is busy. Try again shortly.");
      active++; admitted = true;
      const body = request.url?.endsWith("/upload") ? {} : await read(request);
      for (const [key, value] of sessions) if (value.expires <= Date.now() && value.active === 0) sessions.delete(key);
      if (request.url === "/session") {
        fields(body, ["code"]);
        if (!/^[A-Za-z0-9_-]{43}$/.test(string(body.code, 43))) throw new GatewayError(400, "Invalid launch code.");
        if (sessions.size >= 4096) throw new GatewayError(429, "Mail is busy. Try again shortly.");
        let launch: RecordValue;
        try { launch = record(await options.control({ action: "exchange", code: body.code })); }
        catch { throw new GatewayError(401, "Mail launch expired or was revoked."); }
        const session: Session = { scopeID: identifier(launch.scopeID), customerID: identifier(launch.customerID), mailboxID: identifier(launch.mailboxID), token: string(launch.token, 43), expires: expires(launch.expiresAt), active: 0, uploadedBytes: 0, attachments: new Map() };
        if (!/^[A-Za-z0-9_-]{43}$/.test(session.token)) throw new GatewayError(502, "Invalid Mail session.");
        const cookie = randomBytes(32).toString("base64url"); sessions.set(cookie, session);
        response.setHeader("Set-Cookie", `__Secure-webdock_mail=${cookie}; Path=/s/${session.scopeID}/; HttpOnly; Secure; SameSite=Strict; Max-Age=${Math.floor((session.expires-Date.now())/1000)}`);
        json(response, 200, { scopeID: session.scopeID, customerID: session.customerID, mailboxID: session.mailboxID, expiresAt: launch.expiresAt }); return;
      }
      const path = request.url?.match(/^\/s\/([1-9][0-9]{0,18})\/(mail|logout|upload|download)$/);
      const cookie = request.headers.cookie?.split(";").map(value=>value.trim()).find(value=>value.startsWith("__Secure-webdock_mail="))?.slice("__Secure-webdock_mail=".length);
      const session = cookie ? sessions.get(cookie) : undefined;
      if (!path || !session || session.scopeID !== path[1] || session.expires <= Date.now()) throw new GatewayError(401, "Mail session required.");
      if (session.active >= 4) throw new GatewayError(429, "Too many Mail requests.");
      session.active++; running = session;
      if (path[2] === "logout") {
        fields(body, []); sessions.delete(cookie!);
        await options.control({ action: "logout", token: session.token });
        response.setHeader("Set-Cookie", `__Secure-webdock_mail=; Path=/s/${session.scopeID}/; HttpOnly; Secure; SameSite=Strict; Max-Age=0`);
        json(response, 200, { ok: true }); return;
      }
      let access: RecordValue;
      try { access = record(await options.control({ action: "inspect", token: session.token })); }
      catch { throw new GatewayError(401, "Mail access expired or was revoked."); }
      if (access.scopeID !== session.scopeID || access.customerID !== session.customerID || access.mailboxID !== session.mailboxID) throw new GatewayError(403, "Mail scope does not match.");
      expires(access.expiresAt);
      const runtimeOrigin = `http://app-${session.customerID}.wd-mail-${session.customerID}.svc.cluster.local:8080`;
      if (access.runtimeOrigin !== (options.testRuntimeOrigin ?? runtimeOrigin)) throw new GatewayError(403, "Mail runtime is not authorized.");
      const client = new JmapClient({ url: string(access.runtimeOrigin, 300), accountId: string(access.accountID), authorization: string(access.authorization, 4096), sender: { email: string(access.senderAddress, 320), name: string(access.senderName, 80) }, allowInsecureHttp: true });
      if(path[2] === "upload") {
        if(session.attachments.size>=20 || session.uploadedBytes>=2*MAX_ATTACHMENT_BYTES)throw new GatewayError(413,"Mail attachment limit reached.");
        let name:string;try{name=decodeURIComponent(string(request.headers["x-webdock-filename"],600));}catch{throw new GatewayError(400,"Invalid attachment filename.");}
        if(!name || name.length>200 || /[\r\n\0/\\]/.test(name))throw new GatewayError(400,"Invalid attachment filename.");
        const type=string(request.headers["content-type"],127);
        const parts:Buffer[]=[];let size=0;
        for await(const part of request){size+=part.length;if(size>MAX_ATTACHMENT_BYTES || session.uploadedBytes+size>2*MAX_ATTACHMENT_BYTES)throw new GatewayError(413,"Mail attachment is too large.");parts.push(Buffer.from(part));}
        session.uploadedBytes+=size;
        const attachment={...await client.upload(Buffer.concat(parts),type),name};session.attachments.set(attachment.blobId,attachment);
        json(response,200,{data:attachment});return;
      }
      if(path[2] === "download") {
        fields(body,["emailId","blobId"]);
        const attachment=await client.downloadAttachment(string(body.emailId),string(body.blobId));
        const filename=attachment.name.replace(/[\r\n\0/\\]/g,"_").slice(0,200)||"attachment";
        response.writeHead(200,{"Content-Type":"application/octet-stream","Content-Length":attachment.bytes.length,"Content-Disposition":`attachment; filename="attachment"; filename*=UTF-8''${encodeURIComponent(filename).replace(/['()*]/g,char=>'%'+char.charCodeAt(0).toString(16))}`,"Cache-Control":"no-store","X-Content-Type-Options":"nosniff","Content-Security-Policy":"default-src 'none'; sandbox"});response.end(attachment.bytes);return;
      }
      const write = async (action: "draft" | "submit" | "update", payload: RecordValue, resourceID: string | undefined, perform: () => Promise<unknown>) => {
        if (!/^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/i.test(string(body.requestID, 36))) throw new GatewayError(400, "Invalid Mail request ID.");
        const reservation = record(await options.control({ action: "reserve", write: { token: session.token, requestID: body.requestID, action, ...(resourceID ? { resourceID } : {}), payloadHash: createHash("sha256").update(JSON.stringify(payload)).digest("hex") } }));
        if (reservation.execute !== true) {
          if (reservation.state === "succeeded") return reservation.result;
          throw new GatewayError(409, reservation.state === "rejected" ? "Mail change was rejected." : "Mail change outcome is unknown. Check the mailbox before taking further action.", reservation.state !== "rejected");
        }
        const operationID = identifier(reservation.operationID), leaseToken = string(reservation.leaseToken, 36);
        let result: unknown;
        try { result = await perform(); }
        catch (error) {
          const uncertain = error instanceof GatewayError ? error.uncertain : !(error instanceof JmapError) || error.uncertain;
          await options.control({ action: "finish", write: { token: session.token, operationID, leaseToken, state: uncertain ? "uncertain" : "rejected" } }).catch(()=>{});
          throw error;
        }
        try { await options.control({ action: "finish", write: { token: session.token, operationID, leaseToken, state: "succeeded", result } }); }
        catch { throw new GatewayError(502, "Mail change outcome could not be recorded. Do not repeat it.", true); }
        return result;
      };
      let data: unknown;
      switch (body.action) {
        case "folders": fields(body, ["action"]); data = await client.mailboxes(); break;
        case "identities": fields(body, ["action"]); data = (await client.identities()).filter(item=>item.email===access.senderAddress).map(item=>({ id: item.id, email: item.email, name: access.senderName })); break;
        case "list": {
          fields(body, ["action", "mailboxId", "text", "position", "limit"]);
          if (body.position !== undefined && typeof body.position !== "number" || body.limit !== undefined && typeof body.limit !== "number") throw new GatewayError(400, "Invalid Mail page.");
          data = await client.emails({ mailboxId: body.mailboxId === undefined ? undefined : string(body.mailboxId), text: body.text === undefined ? undefined : string(body.text, 500), position: body.position as number | undefined, limit: body.limit as number | undefined }); break;
        }
        case "read": fields(body, ["action", "emailId"]); data = await client.email(string(body.emailId)); break;
        case "draft": {
          fields(body, ["action", "requestID", "draft", "previousId"]);
          const draft = record(body.draft); fields(draft, ["identityId", "to", "cc", "bcc", "subject", "text", "inReplyTo", "references", "attachments"]);
          const previousId=body.previousId===undefined?undefined:string(body.previousId);
          data = await write("draft", draft, previousId, async () => {
            const previous=previousId?await client.email(previousId):undefined;
            const previousAttachments=Array.isArray(previous?.attachments)?previous.attachments:[];
            if(draft.attachments!==undefined){
              if(!Array.isArray(draft.attachments) || draft.attachments.length>10)throw new GatewayError(400,"Invalid Mail attachments.");
              for(const item of draft.attachments){const attachment=record(item),owned=session.attachments.get(string(attachment.blobId))??previousAttachments.find(item=>record(item).blobId===attachment.blobId);if(!owned || owned.name!==attachment.name || owned.type!==attachment.type || owned.size!==attachment.size)throw new GatewayError(403,"Mail attachment is not authorized for this session.");}
            }
            return client.createDraft(draft as MailDraft,previousId);
          }); break;
        }
        case "submission-lookup": {
          fields(body,['action','emailId']);if(!options.nativeSending)throw new GatewayError(503,'Mail sending is not available yet.');
          const value=await options.nativeSending({action:'lookup',token:session.token,emailID:string(body.emailId)});
          if(value===null){data=null;break;}
          data=nativeReceipt(value);break;
        }
        case "submission": {
          fields(body,['action','id','operation']);
          if(!options.nativeSending || !['status','cancel','file'].includes(String(body.operation)))throw new GatewayError(400,'Unsupported Mail operation.');
          const receipt=record(await options.nativeSending({action:body.operation,token:session.token,id:identifier(body.id)}));
          data=nativeReceipt(receipt);break;
        }
        case "submit": {
          fields(body,['action','requestID','emailId','identityId']);
          if(options.nativeSending){
            if(!/^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/i.test(string(body.requestID,36)))throw new GatewayError(400,'Invalid Mail request ID.');
            let receipt:RecordValue;
            try{receipt=record(await options.nativeSending({action:'submit',token:session.token,requestID:body.requestID,emailID:string(body.emailId),identityID:string(body.identityId)}));}
            catch{throw new GatewayError(503,'Mail submission acknowledgement is unavailable. Check sending status before retrying.',true);}
            data=nativeReceipt(receipt);break;
          }
          // The isolated legacy protocol test cannot enable a production JMAP sending bypass.
          if (!options.testDirectSubmission) throw new GatewayError(503, "Mail sending is not available yet.");
          fields(body, ["action", "requestID", "emailId", "identityId"]);
          const emailId = string(body.emailId), identityId = string(body.identityId);
          const receipt = record(await write("submit", { emailId, identityId }, emailId, () => client.submitDraft(emailId, identityId)));
          let filed = true;
          try { await client.fileSent(emailId); } catch { filed = false; }
          data = { ...receipt, filed }; break;
        }
        case "update": {
          fields(body, ["action", "requestID", "emailId", "change"]);
          const emailId = string(body.emailId), change = record(body.change); fields(change, ["seen", "flagged", "mailboxIds"]);
          data = await write("update", { emailId, change }, emailId, () => client.updateEmail(emailId, change)); break;
        }
        default: throw new GatewayError(400, "Unsupported Mail operation.");
      }
      json(response, 200, { data });
    } catch (error) {
      const status = error instanceof GatewayError ? error.status : error instanceof JmapError ? 502 : 503;
      json(response, status, { error: (error instanceof GatewayError || error instanceof JmapError) ? error.message : "Mail request failed.", ...((error instanceof JmapError || error instanceof GatewayError) && error.uncertain ? { uncertain: true } : {}) });
    } finally { if (running) running.active--; if (admitted) active--; }
  });
  server.requestTimeout = 30_000; server.headersTimeout = 10_000;
  return server;
}
