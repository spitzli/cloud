import { connect } from "node:tls";
import { once } from "node:events";
import { createHash, timingSafeEqual } from "node:crypto";

export class SMTPFailure extends Error {
  readonly uncertain: boolean;
  readonly responseCode?: number;
  constructor(uncertain: boolean, responseCode?: number) {
    super(uncertain ? "SMTP acceptance is unknown. Do not submit again." : "SMTP did not accept the message.");
    this.uncertain=uncertain;this.responseCode=responseCode;
  }
}
type Options={host:string;port:number;certificateSha256:string;username:string;password:string;timeoutMs?:number;allowLoopback?:boolean};
type Message={from:string;to:string[];raw:Buffer};
type Reply={code:number;lines:string[]};
const mailbox=(value:string)=>typeof value==="string" && value.length<=320 && /^[^\s<>@,;:\x00-\x1f]+@(?:[a-z0-9-]+\.)+[a-z0-9-]+$/i.test(value);

/** Single attempt to the assigned private Stalwart instance; its certificate is pinned before AUTH. */
export async function submitSMTP(options:Options,message:Message):Promise<{accepted:true}> {
  const local=options.allowLoopback===true && process.env.NODE_ENV!=="production" && options.host==="127.0.0.1";
  if ((!local && (!/^app-([1-9][0-9]{0,18})\.wd-mail-\1\.svc\.cluster\.local$/.test(options.host) || options.port!==2465)) || !Number.isSafeInteger(options.port) || options.port<1 || options.port>65535 || !/^[a-f0-9]{64}$/.test(options.certificateSha256) || [options.username,options.password].some(value=>typeof value!=="string" || !value || value.length>1024 || /[\x00-\x1f]/.test(value)))throw new SMTPFailure(false);
  if (!mailbox(message.from) || !Array.isArray(message.to) || !message.to.length || message.to.length>100 || !message.to.every(mailbox) || !Buffer.isBuffer(message.raw) || message.raw.length>25*1024*1024 || message.raw.includes(0))throw new SMTPFailure(false);
  const raw=message.raw.toString("latin1");
  if (!raw.includes("\r\n\r\n") || /(^|[^\r])\n|\r(?!\n)/.test(raw))throw new SMTPFailure(false);
  const timeout=options.timeoutMs??30_000;
  if(!Number.isSafeInteger(timeout) || timeout<100 || timeout>120_000)throw new SMTPFailure(false);
  let dataStarted=false,failure:SMTPFailure|undefined;
  let waiting:{resolve:(value:Reply)=>void;reject:(error:SMTPFailure)=>void}|undefined;
  const replies:Reply[]=[];let pending="",lines:string[]=[],replyCode=0,replyBytes=0;
  // CA validation is replaced by an exact DER pin supplied by the verified instance controller.
  const socket=connect({host:options.host,port:options.port,rejectUnauthorized:false});
  const fail=()=>{failure??=new SMTPFailure(dataStarted);waiting?.reject(failure);waiting=undefined;};
  socket.on("error",fail);socket.on("close",fail);
  socket.on("data",chunk=>{
    pending+=chunk.toString("ascii");
    if(pending.length+replyBytes>16384){fail();socket.destroy();return;}
    for(;;){
      const index=pending.indexOf("\r\n");if(index<0)break;
      const line=pending.slice(0,index);pending=pending.slice(index+2);
      const match=/^([0-9]{3})([ -])(.*)$/.exec(line);
      if(!match || replyCode && replyCode!==Number(match[1])){fail();socket.destroy();return;}
      replyCode=Number(match[1]);lines.push(match[3]);replyBytes+=line.length;
      if(lines.length>100){fail();socket.destroy();return;}
      if(match[2]===" "){
        const reply={code:replyCode,lines};lines=[];replyCode=0;replyBytes=0;
        if(waiting){const current=waiting;waiting=undefined;current.resolve(reply);}else replies.push(reply);
        if(replies.length>8){fail();socket.destroy();return;}
      }
    }
  });
  const read=():Promise<Reply>=>{
    if(failure)return Promise.reject(failure);
    const reply=replies.shift();if(reply)return Promise.resolve(reply);
    return new Promise((resolve,reject)=>{waiting={resolve,reject};});
  };
  const expected=(reply:Reply,code:number)=>{
    if(reply.code!==code)throw new SMTPFailure(reply.code>=400 && reply.code<600?false:dataStarted,reply.code);
    return reply;
  };
  const command=async(line:string,code:number)=>{socket.write(line+"\r\n");return expected(await read(),code);};
  const timer=setTimeout(()=>socket.destroy(new Error("SMTP deadline")),timeout);
  try{
    await once(socket,"secureConnect");
    const certificate=socket.getPeerCertificate().raw;
    if(!certificate || !timingSafeEqual(createHash("sha256").update(certificate).digest(),Buffer.from(options.certificateSha256,"hex")))throw new SMTPFailure(false);
    expected(await read(),220);
    const capabilities=(await command("EHLO webdock.invalid",250)).lines;
    if(!capabilities.some(value=>/^AUTH\s+.*\bPLAIN\b/i.test(value)))throw new SMTPFailure(false);
    await command("AUTH PLAIN "+Buffer.from(`\0${options.username}\0${options.password}`).toString("base64"),235);
    const utf8=/[^\x00-\x7f]/.test(raw+message.from+message.to.join(""));
    if(utf8 && !capabilities.some(value=>/^SMTPUTF8\b/i.test(value)))throw new SMTPFailure(false);
    const eightBit=/[^\x00-\x7f]/.test(raw);
    if(eightBit && !capabilities.some(value=>/^8BITMIME\b/i.test(value)))throw new SMTPFailure(false);
    await command(`MAIL FROM:<${message.from}>${utf8?" SMTPUTF8":""}${eightBit?" BODY=8BITMIME":""}`,250);
    for(const recipient of [...new Set(message.to)])await command(`RCPT TO:<${recipient}>`,250);
    await command("DATA",354);
    dataStarted=true;
    const content=raw.replace(/(^|\r\n)\./g,"$1..");
    socket.write(Buffer.from(content+(content.endsWith("\r\n")?"":"\r\n")+".\r\n","latin1"));
    expected(await read(),250);
    return {accepted:true};
  }catch(error){throw error instanceof SMTPFailure?error:new SMTPFailure(dataStarted);}
  finally{clearTimeout(timer);socket.destroy();}
}
