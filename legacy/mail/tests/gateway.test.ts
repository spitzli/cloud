import test from "node:test";
import assert from "node:assert/strict";
import { once } from "node:events";
import { createServer } from "node:http";
import { createMailGateway } from "../src/gateway/server.ts";

test("Webmail gateway pins mailbox sessions, authenticates every read, rejects CSRF and hides credentials", async () => {
  let revoked = false, reads = 0;
  const upstream = createServer(async (req, res) => {
    assert.equal(req.headers.authorization, "Basic private");
    const chunks: Buffer[] = []; for await (const part of req) chunks.push(part);
    if(req.url?.startsWith('/jmap/upload/')){res.end(JSON.stringify({accountId:'group-a',blobId:'attachment1',size:5,type:'text/plain'}));return;}
    const [method, args, tag] = JSON.parse(Buffer.concat(chunks).toString()).methodCalls[0];
    assert.equal(args.accountId, "group-a"); reads++;
    res.setHeader("Content-Type", "application/json");
    res.end(JSON.stringify({ methodResponses: [[method, { accountId: args.accountId, list: [{ id: "inbox", role: "inbox", name: "Inbox" }] }, tag]] }));
  });
  upstream.listen(0, "127.0.0.1"); await once(upstream, "listening");
  const address = upstream.address(); assert.ok(address && typeof address !== "string");
  const runtimeOrigin = `http://127.0.0.1:${address.port}`;
  const gateway = createMailGateway({ allowedOrigins: ["https://studio.webdock.test"],
    testRuntimeOrigin: runtimeOrigin,
    control: async body => {
      if (revoked) throw Error("private backend failure");
      if (body.action === "exchange") return { token: "t".repeat(43), scopeID: "123", customerID: "12", mailboxID: "34", expiresAt: new Date(Date.now()+60000).toISOString() };
      if (body.action === "reserve") return {execute:true,operationID:'456',leaseToken:'11111111-1111-4111-8111-111111111111'};
      return { scopeID: "123", customerID: "12", mailboxID: "34", subject: "56", accountID: "group-a", senderName: "Support", senderAddress: "support@example.com", runtimeOrigin, authorization: "Basic private", expiresAt: new Date(Date.now()+60000).toISOString() };
    } });
  gateway.listen(0, "127.0.0.1"); await once(gateway, "listening");
  const ga = gateway.address(); assert.ok(ga && typeof ga !== "string");
  const call = (path: string, body: unknown, cookie = "", origin = "https://studio.webdock.test") => fetch(`http://127.0.0.1:${ga.port}`+path, { method: "POST", headers: { Origin: origin, "Content-Type": "application/json", Cookie: cookie }, body: JSON.stringify(body) });
  try {
    assert.equal((await call("/session", { code: "x".repeat(43) }, "", "https://evil.test")).status, 403);
    const launch = await call("/session", { code: "x".repeat(43) });
    assert.equal(launch.status, 200);
    const cookieHeader = launch.headers.get("set-cookie")!;
    assert.match(cookieHeader, /Path=\/s\/123\//); assert.match(cookieHeader, /HttpOnly/); assert.match(cookieHeader, /Secure/);
    const cookie = cookieHeader.split(";")[0];
    const launchData = await launch.json(); assert.equal(launchData.scopeID, "123");
    assert.ok(!JSON.stringify(launchData).includes("token"));
    const folders = await call("/s/123/mail", { action: "folders" }, cookie);
    assert.equal(folders.status, 200); assert.ok(!(await folders.text()).includes("private"));
    assert.equal((await call("/s/124/mail", { action: "folders" }, cookie)).status, 401);
    assert.equal((await call("/s/123/mail", { action: "folders", accountId: "foreign" }, cookie)).status, 400);
    const upload=await fetch(`http://127.0.0.1:${ga.port}/s/123/upload`,{method:'POST',headers:{Origin:'https://studio.webdock.test','Content-Type':'text/plain','X-Webdock-Filename':'note.txt',Cookie:cookie},body:'hello'});
    assert.equal(upload.status,200);assert.equal((await upload.json()).data.blobId,'attachment1');
    assert.equal((await call('/s/123/mail',{action:'draft',requestID:'11111111-1111-4111-8111-111111111111',draft:{attachments:[{blobId:'foreign',name:'note.txt',size:5,type:'text/plain'}]}},cookie)).status,403);
    revoked = true;
    const denied = await call("/s/123/mail", { action: "folders" }, cookie);
    assert.equal(denied.status, 401); assert.ok(!(await denied.text()).includes("private"));
    assert.equal(reads, 1);
  } finally { gateway.closeAllConnections(); upstream.closeAllConnections(); await Promise.all([new Promise<void>(resolve=>gateway.close(()=>resolve())), new Promise<void>(resolve=>upstream.close(()=>resolve()))]); }
});

test("gateway reserves writes before JMAP and never resubmits after a lost result", async () => {
  let submitted = 0, reserved = false;
  const upstream = createServer(async (req, res) => {
    const chunks: Buffer[] = []; for await (const part of req) chunks.push(part);
    const [method, args, tag] = JSON.parse(Buffer.concat(chunks).toString()).methodCalls[0];
    let result: Record<string, unknown>;
    if (method === 'Identity/get') result = {list:[{id:'sender',email:'support@example.com'}]};
    else if (method === 'Email/get') result = {list:[{id:'draft1',from:[{email:'support@example.com'}],keywords:{$draft:true}}]};
    else { assert.equal(method,'EmailSubmission/set'); assert.equal(reserved,true); submitted++; res.destroy(); return; }
    res.end(JSON.stringify({methodResponses:[[method,{...result,accountId:args.accountId},tag]]}));
  });
  upstream.listen(0,'127.0.0.1'); await once(upstream,'listening');
  const ua=upstream.address(); assert.ok(ua && typeof ua!=='string');
  const runtimeOrigin=`http://127.0.0.1:${ua.port}`;
  const gateway=createMailGateway({allowedOrigins:['https://studio.webdock.test'],testRuntimeOrigin:runtimeOrigin,testDirectSubmission:true,control:async body=>{
    const expiresAt=new Date(Date.now()+60000).toISOString();
    if(body.action==='exchange')return {token:'t'.repeat(43),scopeID:'123',customerID:'12',mailboxID:'34',expiresAt};
    if(body.action==='reserve'){ const execute=!reserved;reserved=true;return {execute,operationID:'456',leaseToken:'11111111-1111-4111-8111-111111111111',state:execute?'reserved':'uncertain',result:null}; }
    if(body.action==='finish')return {ok:true};
    return {scopeID:'123',customerID:'12',mailboxID:'34',subject:'56',accountID:'group-a',senderName:'Support',senderAddress:'support@example.com',runtimeOrigin,authorization:'Basic private',expiresAt};
  }});
  gateway.listen(0,'127.0.0.1');await once(gateway,'listening');const ga=gateway.address();assert.ok(ga && typeof ga!=='string');
  const call=(path:string,body:unknown,cookie='')=>fetch(`http://127.0.0.1:${ga.port}`+path,{method:'POST',headers:{Origin:'https://studio.webdock.test','Content-Type':'application/json',Cookie:cookie},body:JSON.stringify(body)});
  try {
    const launch=await call('/session',{code:'x'.repeat(43)}),cookie=launch.headers.get('set-cookie')!.split(';')[0];
    const command={action:'submit',emailId:'draft1',identityId:'sender',requestID:'11111111-1111-4111-8111-111111111111'};
    const first=await call('/s/123/mail',command,cookie);assert.equal(first.status,502);assert.equal((await first.json()).uncertain,true);
    const second=await call('/s/123/mail',command,cookie);assert.equal(second.status,409);assert.equal((await second.json()).uncertain,true);
    assert.equal(submitted,1);
  }finally{gateway.closeAllConnections();upstream.closeAllConnections();await Promise.all([new Promise<void>(r=>gateway.close(()=>r())),new Promise<void>(r=>upstream.close(()=>r()))]);}
});

test('production Webmail submission delegates its bound session to the queue and never calls JMAP submission',async()=>{
 const forwarded:Record<string,unknown>[]=[];
 const gateway=createMailGateway({allowedOrigins:['https://studio.webdock.test'],control:async body=>{
  const expiresAt=new Date(Date.now()+60000).toISOString();
  if(body.action==='exchange')return {token:'t'.repeat(43),scopeID:'123',customerID:'12',mailboxID:'34',expiresAt};
  assert.equal(body.action,'inspect');return {scopeID:'123',customerID:'12',mailboxID:'34',subject:'56',accountID:'group',senderName:'Support',senderAddress:'support@example.test',runtimeOrigin:'http://app-12.wd-mail-12.svc.cluster.local:8080',authorization:'Basic private',expiresAt};
 },nativeSending:async body=>{forwarded.push(body);return {id:'789',state:'queued',filed:false,authorization:'must-not-leak',filingState:{private:'must-not-leak'}};}});
 gateway.listen(0,'127.0.0.1');await once(gateway,'listening');const port=(gateway.address() as {port:number}).port;
 const call=(path:string,body:unknown,cookie='')=>fetch(`http://127.0.0.1:${port}`+path,{method:'POST',headers:{Origin:'https://studio.webdock.test','Content-Type':'application/json',Cookie:cookie},body:JSON.stringify(body)});
 try{
  const launch=await call('/session',{code:'x'.repeat(43)}),cookie=launch.headers.get('set-cookie')!.split(';')[0];
  const result=await call('/s/123/mail',{action:'submit',requestID:'11111111-1111-4111-8111-111111111111',emailId:'draft',identityId:'sender'},cookie);
  assert.equal(result.status,200);assert.deepEqual(await result.json(),{data:{id:'789',state:'queued',filed:false}});
  assert.equal(forwarded.length,1);assert.equal(forwarded[0].token,'t'.repeat(43));assert.equal(forwarded[0].emailID,'draft');
  assert.equal((await call('/s/123/mail',{action:'submit',requestID:crypto.randomUUID(),emailId:'draft',identityId:'sender',customerID:'other'},cookie)).status,400);
  assert.equal(forwarded.length,1);
 }finally{gateway.closeAllConnections();await new Promise<void>(resolve=>gateway.close(()=>resolve()));}
});
