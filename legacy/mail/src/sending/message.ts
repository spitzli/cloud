import { domainToASCII } from "node:url";
import { createHash } from "node:crypto";

const MAX_MESSAGE=25*1024*1024;
function invalid():never{throw Error("Invalid native Mail message.");}
export function envelopeAddress(value:string){
  if(typeof value!=="string" || value.length>320 || /[\s<>()[\]\\",;:\x00-\x1f\x7f]/.test(value))invalid();
  const pieces=value.split("@");if(pieces.length!==2)invalid();
  const local=pieces[0].normalize("NFC").toLowerCase(),domain=domainToASCII(pieces[1]).toLowerCase();
  if(!local || local.split(".").some(part=>!part) || Buffer.byteLength(local)>64 || !domain.includes(".") || domain.length>253 || !domain.split(".").every(part=>/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(part)))invalid();
  const address=`${local}@${domain}`;if(Buffer.byteLength(address)>254)invalid();return address;
}
function displayName(name:string){
  if(typeof name!=="string" || name.length>80 || /[\x00-\x1f\x7f]/.test(name))invalid();
  const words:string[]=[];let current="";
  for(const char of name){if(Buffer.byteLength(current+char)>42){words.push(current);current="";}current+=char;}
  if(current)words.push(current);
  return words.map(word=>`=?UTF-8?B?${Buffer.from(word).toString("base64")}?=`).join("\r\n ");
}

/** Preserve MIME body bytes; envelope recipients are independent from visible headers. */
export function prepareMessage(raw:Buffer,sender:{email:string;name?:string},recipients:string[]){
  if(!Buffer.isBuffer(raw) || raw.length>MAX_MESSAGE || raw.includes(0) || !Array.isArray(recipients) || !recipients.length || recipients.length>100)invalid();
  const from=envelopeAddress(sender.email),to=[...new Set(recipients.map(envelopeAddress))];
  const split=raw.indexOf("\r\n\r\n");if(split<0 || split>131072)invalid();
  const headerText=raw.subarray(0,split).toString("latin1");
  if(/(^|[^\r])\n|\r(?!\n)/.test(headerText))invalid();
  const headers:{name:string;raw:string}[]=[];
  for(const line of headerText.split("\r\n")){
    if(/^[ \t]/.test(line)){if(!headers.length)invalid();headers[headers.length-1].raw+="\r\n"+line;continue;}
    const colon=line.indexOf(":");if(colon<1 || !/^[A-Za-z0-9!#$%&'*+.^_`|~-]+$/.test(line.slice(0,colon)))invalid();
    headers.push({name:line.slice(0,colon).toLowerCase(),raw:line});
    if(headers.length>1000)invalid();
  }
  const visible=headers.filter(header=>!["from","sender","return-path","bcc","dkim-signature","authentication-results","delivered-to","x-original-to"].includes(header.name) && !/^(x-webdock-|resent-|arc-)/.test(header.name));
  const name=sender.name?displayName(sender.name):"";
  const prepared=Buffer.concat([Buffer.from(`From: ${name?name+"\r\n ":""}<${from}>\r\n`,"utf8"),Buffer.from(visible.length?visible.map(header=>header.raw).join("\r\n")+"\r\n\r\n":"\r\n","latin1"),raw.subarray(split+4)]);
  if(prepared.length>MAX_MESSAGE-128)invalid();
  return {from,to,raw:prepared,hash:createHash("sha256").update(prepared).digest("hex")};
}
export function stampMessage(raw:Buffer,submissionID:string,proof:string){
  if(!Buffer.isBuffer(raw) || !/^[1-9][0-9]{0,18}$/.test(submissionID) || !/^[A-Za-z0-9_-]{43}$/.test(proof))invalid();
  const stamped=Buffer.concat([Buffer.from(`X-Webdock-Submission: ${submissionID}.${proof}\r\n`),raw]);
  if(stamped.length>MAX_MESSAGE)invalid();return stamped;
}
