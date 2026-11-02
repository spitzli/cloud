import {mkdtempSync,readFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {X509Certificate} from 'node:crypto';
import {messageDigest,hookMessageDigest} from '@webdock/mail-core/submission-proof';
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { isIP, createServer as tcpServer, connect as tcpConnect, type Socket } from "node:net";
import { connect as tlsConnect } from "node:tls";
import { createHash } from "node:crypto";
import { once } from "node:events";
import { submitSMTP } from "../../src/sending/smtp.ts";
import { prepareMessage } from "../../src/sending/message.ts";
import { verifyFixtureDKIM } from "./dkim-check.ts";
import { setTimeout } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import test from "node:test";
import nodemailer from "nodemailer";
import { JmapClient } from "../../src/stalwart/jmap.ts";
import { StalwartClient } from "../../src/stalwart/client.ts";

const composeFile = fileURLToPath(new URL("../../../../infra/mail/compose.test.yml", import.meta.url));
const project = process.env.MAIL_TEST_PROJECT;
assert.match(project || "", /^webdock-mail-test-[a-z0-9-]+$/, "Use bun run --filter @webdock/mail test:integration.");
const compose = (...args: string[]) => execFileSync("docker", ["compose", "-p", project!, "-f", composeFile, ...args], { encoding: "utf8", timeout: 30_000 });
const basic = (user: string, password: string) => `Basic ${Buffer.from(`${user}:${password}`).toString("base64")}`;
async function eventually<T>(operation: () => Promise<T>): Promise<T> {
  const until = Date.now() + 15_000;
  for (;;) {
    try { return await operation(); }
    catch (error) { if (Date.now() >= until) throw error; await setTimeout(200); }
  }
}

function instance(key: string) {
  const service = `customer-${key}`;
  const id = compose("ps", "-q", service).trim();
  assert.match(id, /^[a-f0-9]{64}$/);
  const [container] = JSON.parse(execFileSync("docker", ["inspect", id], { encoding: "utf8" }));
  assert.equal(container.Config.Labels["com.docker.compose.project"], project);
  const network = container.NetworkSettings.Networks[`${project}_${service}`];
  assert.ok(network);
  const [networkInfo] = JSON.parse(execFileSync("docker", ["network", "inspect", network.NetworkID], { encoding: "utf8" }));
  assert.equal(networkInfo.Internal, true);
  assert.equal(isIP(network.IPAddress), 4);
  const url = `http://${network.IPAddress}:8080`;
  return { key, service, ip: network.IPAddress as string, gateway: networkInfo.IPAM.Config[0].Gateway as string, url, domain: `${service}.test`,
    password: `Webdock-Fixture-Only-${key}!123456`,
    client: new StalwartClient({ url, authorization: basic("admin", `webdock-local-test-only-${key}`), allowInsecureHttp: true }),
  };
}
let instances: ReturnType<typeof instance>[];
let accountIDs: string[];
test.before(async () => {
  instances = [instance("a"), instance("b")];
  accountIDs = [];
  for (const item of instances) {
    await eventually(() => item.client.call("x:Bootstrap/get", { ids: ["singleton"] }));
    await item.client.call("x:Bootstrap/set", { update: { singleton: {
      serverHostname: `mail.${item.domain}`, defaultDomain: item.domain,
      requestTlsCertificate: false, generateDkimKeys: false, tracer: { "@type": "Stdout", level: "warn" },
    } } });
    compose("restart", item.service);
    const domains = await eventually(() => item.client.call("x:Domain/get", { ids: null }));
    const domain = (domains.list as { id: string; name: string }[]).find(value => value.name === item.domain);
    assert.ok(domain);
    const accounts = await item.client.call("x:Account/set", { create: { mailbox: {
      "@type": "User", name: "alice", domainId: domain.id,
      credentials: { "0": { "@type": "Password", secret: item.password } },
      roles: { "@type": "User" }, permissions: { "@type": "Inherit" },
      encryptionAtRest: { "@type": "Disabled" }, quotas: { maxDiskQuota: 10_485_760 },
    } } });
    accountIDs.push((accounts.created as { mailbox: { id: string } }).mailbox.id);
  }
});

test("CE instances have separate domains and reject the other instance's administrator", async () => {
  assert.notEqual(instances[0].url, instances[1].url);
  for (const item of instances) {
    const denied = new StalwartClient({ url: item.url, authorization: basic("admin", `webdock-local-test-only-${item.key === "a" ? "b" : "a"}`), allowInsecureHttp: true });
    await assert.rejects(denied.call("x:Domain/get", { ids: null }), /request failed/);
    const result = await item.client.call("x:Domain/get", { ids: null });
    assert.deepEqual((result.list as { name: string }[]).map(domain => domain.name), [item.domain]);
    const account = await fetch(`${item.url}/api/account`, { headers: { Authorization: basic("admin", `webdock-local-test-only-${item.key}`) }, signal: AbortSignal.timeout(5000) });
    assert.equal((await account.json()).edition, "community");
  }
});

test("a user cannot authenticate against another instance or manage their own server", async () => {
  const [a, b] = instances;
  const foreign = await fetch(`${b.url}/.well-known/jmap`, { headers: { Authorization: basic(`alice@${a.domain}`, a.password) }, signal: AbortSignal.timeout(5000) });
  assert.equal(foreign.status, 401);
  const own = new StalwartClient({ url: a.url, authorization: basic(`alice@${a.domain}`, a.password), allowInsecureHttp: true });
  await assert.rejects(own.call("x:Account/get", { ids: null }), /rejected/);
});

test("authenticated SMTP delivers into JMAP on the same instance without leaking to the other instance", async () => {
  const [a, b] = instances;
  const mailer = nodemailer.createTransport({ host: a.ip, port: 465, secure: true,
    auth: { user: `alice@${a.domain}`, pass: a.password },
    // Only this egress-blocked, disposable fixture uses Stalwart's self-signed certificate.
    tls: { rejectUnauthorized: false }, connectionTimeout: 5000, socketTimeout: 10000,
  });
  try {
    const sent = await mailer.sendMail({ from: `alice@${a.domain}`, to: `alice@${a.domain}`, subject: "Webdock isolation fixture", text: "Only account A may read this message." });
    assert.deepEqual(sent.accepted, [`alice@${a.domain}`]);
    const query = async (item: typeof a, accountId: string) => {
      const response = await fetch(`${item.url}/jmap`, { method: "POST", headers: { "Content-Type": "application/json", Authorization: basic(`alice@${item.domain}`, item.password) },
        body: JSON.stringify({ using: ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"], methodCalls: [["Email/query", { accountId }, "emails"]] }), signal: AbortSignal.timeout(5000) });
      assert.equal(response.status, 200);
      const body = await response.json();
      assert.equal(body.methodResponses[0][0], "Email/query");
      return body.methodResponses[0][1].ids as string[];
    };
    await eventually(async () => assert.equal((await query(a, accountIDs[0])).length, 1));
    assert.equal((await query(b, accountIDs[1])).length, 0);
    await assert.rejects(mailer.sendMail({ from: `alice@${b.domain}`, to: `alice@${a.domain}`, subject: "Forged envelope", text: "Must fail" }));
  } finally { mailer.close(); }
});

test("shared group mail is available only to explicit members and membership revocation removes access", async () => {
  const a = instances[0];
  const domains = await a.client.call("x:Domain/get", { ids: null });
  const domain = (domains.list as { id: string; name: string }[]).find(row => row.name === a.domain)!;
  const created = await a.client.call("x:Account/set", { create: { shared: {
    "@type": "Group", name: "support", domainId: domain.id,
    roles: { "@type": "Default" }, permissions: { "@type": "Inherit" },
    quotas: { maxDiskQuota: 10_485_760 },
  } } });
  const groupID = (created.created as { shared: { id: string } }).shared.id;
  const request = async (method: string, args: Record<string, unknown>) => {
    const response = await fetch(`${a.url}/jmap`, { method: "POST", headers: { "Content-Type": "application/json", Authorization: basic(`alice@${a.domain}`, a.password) },
      body: JSON.stringify({ using: ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail", "urn:ietf:params:jmap:submission"], methodCalls: [[method, args, "shared"]] }), signal: AbortSignal.timeout(5000) });
    assert.equal(response.status, 200);
    return (await response.json()).methodResponses[0];
  };
  assert.equal((await request("Email/query", { accountId: groupID }))[0], "error");
  await a.client.call("x:Account/set", { update: { [accountIDs[0]]: { memberGroupIds: { [groupID]: true } } } });
  await eventually(async () => assert.equal((await request("Mailbox/get", { accountId: groupID, ids: null }))[0], "Mailbox/get"));
  const identities = await request("Identity/get", { accountId: groupID });
  assert.equal(identities[0], "Identity/get");
  assert.ok(identities[1].list.some((identity: { email: string }) => identity.email === `support@${a.domain}`));
  await a.client.call("x:Account/set", { update: { [accountIDs[0]]: { memberGroupIds: {} } } });
  await eventually(async () => assert.equal((await request("Email/query", { accountId: groupID }))[0], "error"));
});


test("the user-scoped Webmail client composes, reads and replies through real JMAP", async () => {
  const a = instances[0];
  const domain = ((await a.client.call("x:Domain/get", { ids: null })).list as { id: string; name: string }[]).find(row => row.name === a.domain)!;
  const bobPassword = "Bob-Disposable-Mail-Fixture-Only!123456";
  const created = await a.client.call("x:Account/set", { create: { bob: {
    "@type": "User", name: "bob", domainId: domain.id,
    credentials: { "0": { "@type": "Password", secret: bobPassword } },
    roles: { "@type": "User" }, permissions: { "@type": "Inherit" },
    encryptionAtRest: { "@type": "Disabled" }, quotas: { maxDiskQuota: 10_485_760 },
  } } });
  const bobId = (created.created as { bob: { id: string } }).bob.id;
  const alice = new JmapClient({ url: a.url, allowInsecureHttp: true, authorization: basic(`alice@${a.domain}`, a.password), accountId: accountIDs[0] });
  const bob = new JmapClient({ url: a.url, allowInsecureHttp: true, authorization: basic(`bob@${a.domain}`, bobPassword), accountId: bobId });
  const aliceIdentity = (await alice.identities())[0].id;
  const attachment = {...await alice.upload(Buffer.from("Native attachment bytes"), "text/plain"), name:"note.txt"};
  const initialDraft = await alice.createDraft({ identityId: aliceIdentity, to: [], subject: "Unfinished draft", text: "Initial text" });
  const draft = await alice.createDraft({ attachments:[attachment], identityId: aliceIdentity, to: [{ email: `bob@${a.domain}` }], subject: "Native Webmail roundtrip", text: "A real message for Bob." },initialDraft.id);
  await assert.rejects(alice.email(initialDraft.id));
  await assert.rejects(alice.createDraft({identityId:aliceIdentity,to:[],subject:"Stale tab",text:"Must not overwrite"},initialDraft.id));
  assert.ok((await alice.submitDraft(draft.id, aliceIdentity)).id);
  await alice.fileSent(draft.id);
  const sent = (await alice.mailboxes()).list.find(row => row.role === "sent")!;
  const filed = await alice.email(draft.id);
  assert.equal((filed.keywords as Record<string,boolean>).$draft, undefined);
  assert.equal((filed.mailboxIds as Record<string,boolean>)[sent.id], true);
  const incoming = await eventually(async () => {
    const row = (await bob.emails({ text: "Native Webmail roundtrip" })).list[0]; assert.ok(row); return bob.email(row.id);
  });
  assert.equal(incoming.subject, "Native Webmail roundtrip");
  const receivedAttachment = (incoming.attachments as {blobId:string}[])[0];
  assert.equal((await bob.downloadAttachment(incoming.id, receivedAttachment.blobId)).bytes.toString(), "Native attachment bytes");
  assert.ok(Object.values(incoming.bodyValues as Record<string, { value: string }>).some(part => part.value.includes("A real message for Bob.")));
  await bob.updateEmail(incoming.id, { seen: true });
  assert.equal(((await bob.email(incoming.id)).keywords as Record<string, boolean>).$seen, true);
  const bobIdentity = (await bob.identities())[0].id;
  const reply = await bob.createDraft({ identityId: bobIdentity, to: [{ email: `alice@${a.domain}` }], subject: "Re: Native Webmail roundtrip", text: "Bob replies inside Webdock.", inReplyTo: incoming.messageId as string[], references: incoming.messageId as string[] });
  assert.ok((await bob.submitDraft(reply.id, bobIdentity)).id);
  await eventually(async () => {
    const received = (await alice.emails({ text: "Bob replies inside Webdock." })).list[0]; assert.ok(received);
    assert.deepEqual((await alice.email(received.id)).inReplyTo, incoming.messageId);
  });
  const denied = new JmapClient({ url: a.url, allowInsecureHttp: true, authorization: basic(`bob@${a.domain}`, bobPassword), accountId: accountIDs[0] });
  await assert.rejects(denied.emails());
});

test('native SMTP transport pins the real isolated CE certificate and submits once',async()=>{
  const a=instances[0];
  const probe=tlsConnect({host:a.ip,port:465,rejectUnauthorized:false});
  await once(probe,'secureConnect');const certificateSha256=createHash('sha256').update(probe.getPeerCertificate().raw).digest('hex');probe.destroy();
  const sockets=new Set<Socket>();
  const proxy=tcpServer(socket=>{const backend=tcpConnect({host:a.ip,port:465});sockets.add(socket);sockets.add(backend);socket.pipe(backend).pipe(socket);socket.on('error',()=>backend.destroy());backend.on('error',()=>socket.destroy());socket.on('close',()=>{sockets.delete(socket);backend.destroy();});backend.on('close',()=>sockets.delete(backend));});
  proxy.listen(0,'127.0.0.1');await once(proxy,'listening');const address=proxy.address();assert.ok(address && typeof address!=='string');
  try{
    const client=new JmapClient({url:a.url,allowInsecureHttp:true,authorization:basic(`alice@${a.domain}`,a.password),accountId:accountIDs[0]});
    const identity=(await client.identities())[0].id;
    const draft=await client.createDraft({identityId:identity,to:[{email:`alice@${a.domain}`}],bcc:[{email:`bob@${a.domain}`}],subject:'Native SMTP fixture',text:'A pinned single attempt.'});
    const exported=await client.exportDraft(draft.id,identity),prepared=prepareMessage(exported.raw,exported.sender,exported.to);
    await submitSMTP({host:'127.0.0.1',port:address.port,certificateSha256,username:`alice@${a.domain}`,password:a.password,allowLoopback:true},prepared);
    await client.fileSent(draft.id);
    const inbox=(await client.mailboxes()).list.find(row=>row.role==='inbox')!;
    const received=await eventually(async()=>{const messages=(await client.emails({text:'Native SMTP fixture',mailboxId:inbox.id})).list;assert.equal(messages.length,1);return client.email(messages[0].id);});
    assert.equal(received.bcc,null);
  }finally{for(const socket of sockets)socket.destroy();await new Promise<void>(resolve=>proxy.close(()=>resolve()));}
});

test('CE generates Webdock-branded DKIM keys and signs a verifiable outgoing message',async()=>{
  const a=instances[0];const domain=((await a.client.call('x:Domain/get',{ids:null})).list as {id:string;name:string}[]).find(row=>row.name===a.domain)!;
  await a.client.call('x:Domain/set',{update:{[domain.id]:{dkimManagement:{'@type':'Automatic',algorithms:{Dkim1RsaSha256:true},selectorTemplate:'webdock-{algorithm}-{date-%Y%m%d}'}}}});
  const keys=await eventually(async()=>{const keys=((await a.client.call('x:DkimSignature/get',{ids:null})).list as {domainId:string;selector:string;publicKey:string}[]).filter(key=>key.domainId===domain.id && key.selector.startsWith('webdock-'));assert.ok(keys.length);return keys;});
  compose('restart',a.service);await eventually(()=>a.client.call('x:Domain/get',{ids:[domain.id]}));
  const mailer=nodemailer.createTransport({host:a.ip,port:465,secure:true,auth:{user:`alice@${a.domain}`,pass:a.password},tls:{rejectUnauthorized:false},connectionTimeout:5000,socketTimeout:10000});
  try{
    await mailer.sendMail({from:`alice@${a.domain}`,to:`alice@${a.domain}`,subject:'Native DKIM fixture',text:'Signed by Webdock DKIM.'});
    const client=new JmapClient({url:a.url,allowInsecureHttp:true,authorization:basic(`alice@${a.domain}`,a.password),accountId:accountIDs[0]});
    const email=await eventually(async()=>{const email=(await client.emails({text:'Native DKIM fixture'})).list[0];assert.ok(email);return client.email(email.id);});
    const raw=Buffer.from(await (await fetch(`${a.url}/jmap/download/${accountIDs[0]}/${email.blobId}/message.eml`,{headers:{Authorization:basic(`alice@${a.domain}`,a.password)}})).arrayBuffer());
    verifyFixtureDKIM(raw,keys,a.domain);
    assert.throws(()=>verifyFixtureDKIM(Buffer.from(raw.toString().replace('Signed by Webdock DKIM.','Altered body.')),keys,a.domain));
  }finally{mailer.close();}
});

test('CE DATA hook exposes authenticated sender context and fails closed when the policy service is unavailable',async()=>{
  const a=instances[0];
  const policyID=compose('ps','-q','policy').trim();
  const [policy]=JSON.parse(execFileSync('docker',['inspect',policyID],{encoding:'utf8'}));
  const policyIP=policy.NetworkSettings.Networks[`${project}_customer-a`].IPAddress;assert.equal(isIP(policyIP),4);
  const policyOrigin=`http://${policyIP}:8090`;
  await eventually(async()=>assert.equal((await fetch(policyOrigin+'/healthz')).status,200));
  const created=await a.client.call('x:MtaHook/set',{create:{policy:{url:`${policyOrigin}/policy`,enable:{else:'true'},stages:{data:true},tempFailOnError:true,timeout:3000,maxResponseSize:4096,allowInvalidCerts:false,httpAuth:{'@type':'Bearer',bearerToken:{'@type':'Value',secret:'disposable-policy-token'}},httpHeaders:{}}}});
  const hookID=(created.created as {policy:{id:string}}).policy.id;
  compose('restart',a.service);
  await eventually(()=>a.client.call('x:MtaHook/get',{ids:[hookID]}));
  const mailer=nodemailer.createTransport({host:a.ip,port:465,secure:true,auth:{user:`alice@${a.domain}`,pass:a.password},tls:{rejectUnauthorized:false},connectionTimeout:5000,socketTimeout:10000});
  try{
    const fixture=Buffer.from(`From: alice@${a.domain}\r\nTo: alice@${a.domain}\r\nSubject: Policy denial\r\n folded fixture\r\n\r\nMust be rejected before acceptance.\r\n`);
    await assert.rejects(mailer.sendMail({envelope:{from:`alice@${a.domain}`,to:[`alice@${a.domain}`]},raw:fixture}),error=>(error as {responseCode?:number}).responseCode===550);
    const captured=await (await fetch(policyOrigin+'/captured')).json();
    assert.ok(captured);assert.equal(hookMessageDigest(captured.message),messageDigest(fixture));const context=captured.context as Record<string,unknown>;
    assert.deepEqual(context.sasl,{login:`alice@${a.domain}`});assert.equal(context.stage,'data');
    const envelope=captured.envelope as {from:{address:string};to:{address:string}[]};assert.equal(envelope.from.address,`alice@${a.domain}`);assert.equal(envelope.to[0].address,`alice@${a.domain}`);
    await fetch(policyOrigin+'/unavailable',{method:'POST'});
    await assert.rejects(mailer.sendMail({from:`alice@${a.domain}`,to:`alice@${a.domain}`,subject:'Policy outage fixture',text:'Must not bypass the unavailable policy'}),error=>{const code=(error as {responseCode?:number}).responseCode;return !!code && code>=400 && code<500;});
    await fetch(policyOrigin+'/accept',{method:'POST'});
    await mailer.sendMail({from:`alice@${a.domain}`,to:`alice@${a.domain}`,subject:'Hook sanitization fixture',text:'Accepted after a policy check.',headers:{'X-Webdock-Submission':'private-fixture-proof'}});
    const client=new JmapClient({url:a.url,allowInsecureHttp:true,authorization:basic(`alice@${a.domain}`,a.password),accountId:accountIDs[0]});
    await eventually(async()=>{const email=(await client.emails({text:'Hook sanitization fixture'})).list[0];assert.ok(email);
      const response=await fetch(a.url+'/jmap',{method:'POST',headers:{'Content-Type':'application/json',Authorization:basic(`alice@${a.domain}`,a.password)},body:JSON.stringify({using:['urn:ietf:params:jmap:core','urn:ietf:params:jmap:mail'],methodCalls:[['Email/get',{accountId:accountIDs[0],ids:[email.id],properties:['id','header:X-Webdock-Submission:asText']},'headers']]})});
      assert.equal((await response.json()).methodResponses[0][1].list[0]['header:X-Webdock-Submission:asText'],null);
    });
  }finally{mailer.close();await a.client.call('x:MtaHook/set',{destroy:[hookID]});}
});

test('CE relay delivers to the sink; missing routes expose the upstream MX fallback that network policy must block',async()=>{
  const a=instances[0];const policyID=compose('ps','-q','policy').trim();
  const [policy]=JSON.parse(execFileSync('docker',['inspect',policyID],{encoding:'utf8'}));
  const policyIP=policy.NetworkSettings.Networks[`${project}_customer-a`].IPAddress;assert.equal(isIP(policyIP),4);
  const original=(await a.client.call('x:MtaOutboundStrategy/get',{ids:['singleton']})).list as Record<string,unknown>[];
  const route=await a.client.call('x:MtaRoute/set',{create:{relay:{'@type':'Relay',name:'webdock-fixture',address:policyIP,port:2526,protocol:'smtp',implicitTls:false,allowInvalidCerts:false,authUsername:null,authSecret:{'@type':'None'}}}});
  const routeID=(route.created as {relay:{id:string}}).relay.id;
  const tls=await a.client.call('x:MtaTlsStrategy/set',{create:{fixture:{name:'webdock-fixture',dane:'disable',mtaSts:'disable',startTls:'disable',allowInvalidCerts:false}}});
  const tlsID=(tls.created as {fixture:{id:string}}).fixture.id;
  await a.client.call('x:MtaOutboundStrategy/set',{update:{singleton:{route:{match:{'0':{if:'is_local_domain(rcpt_domain)',then:"'local'"}},else:"'webdock-fixture'"},tls:{else:"'webdock-fixture'"}}}});
  compose('restart',a.service);await eventually(()=>a.client.call('x:MtaRoute/get',{ids:[routeID]}));
  const mailer=nodemailer.createTransport({host:a.ip,port:465,secure:true,auth:{user:`alice@${a.domain}`,pass:a.password},tls:{rejectUnauthorized:false},connectionTimeout:5000,socketTimeout:10000});
  try{
    await mailer.sendMail({from:`alice@${a.domain}`,to:'recipient@remote.example.invalid',subject:'Explicit route fixture',text:'Only the isolated SMTP sink may receive this.'});
    await eventually(async()=>{const received=await (await fetch(`http://${policyIP}:8090/received`)).json();assert.ok(received.some((row:{content:string;to:string[]})=>row.content.includes('Explicit route fixture') && row.to.some(value=>value.includes('recipient@remote.example.invalid'))));});
    await a.client.call('x:MtaOutboundStrategy/set',{update:{singleton:{route:{else:"'missing-webdock-route'",match:{}}}}});
    compose('restart',a.service);await eventually(()=>a.client.call('x:MtaOutboundStrategy/get',{ids:['singleton']}));
    await mailer.sendMail({from:`alice@${a.domain}`,to:'missing@remote.example.invalid',subject:'No route fallback fixture',text:'The isolated lab must block direct delivery.'});
    // Capability finding, not a production safety proof: CE falls back to MX.
    // The lab has no internet egress; production must deny all direct SMTP.
    await eventually(async()=>assert.match(compose('logs','--tail','100',a.service),/Gateway not found/));
    const received=await (await fetch(`http://${policyIP}:8090/received`)).json();assert.ok(!received.some((row:{content:string})=>row.content.includes('No route fallback fixture')));
  }finally{mailer.close();const {id,...fields}=original[0];await a.client.call('x:MtaOutboundStrategy/set',{update:{singleton:fields}});await a.client.call('x:MtaRoute/set',{destroy:[routeID]});await a.client.call('x:MtaTlsStrategy/set',{destroy:[tlsID]});}
});

test('CE submission service can send for a scoped identity without mailbox or management access',async()=>{
 const a=instances[0];
 const domains=(await a.client.call('x:Domain/get',{ids:null})).list as {id:string;name:string}[];
 const domain=domains.find(row=>row.name===a.domain)!;
 const created=await a.client.call('x:Account/set',{create:{service:{'@type':'User',name:'webdock-submit',domainId:domain.id,roles:{'@type':'User'},permissions:{'@type':'Replace',enabledPermissions:{authenticate:true,emailSend:true},disabledPermissions:{}},credentials:{'0':{'@type':'Password',secret:a.password}},encryptionAtRest:{'@type':'Disabled'},quotas:{maxDiskQuota:1048576}}}});
 const serviceID=(created.created as {service:{id:string}}).service.id,login=`webdock-submit@${a.domain}`;
 const original=((await a.client.call('x:MtaStageAuth/get',{ids:['singleton']})).list as Record<string,unknown>[])[0];
 const mailer=nodemailer.createTransport({host:a.ip,port:465,secure:true,auth:{user:login,pass:a.password},tls:{rejectUnauthorized:false},connectionTimeout:5000,socketTimeout:10000});
 try{
  assert.equal(await mailer.verify(),true);
  await assert.rejects(mailer.sendMail({from:`alice@${a.domain}`,to:`alice@${a.domain}`,subject:'Service denied fixture',text:'Must require the scoped sender exception.'}),error=>(error as {responseCode?:number}).responseCode===501);
  await a.client.call('x:MtaStageAuth/set',{update:{singleton:{mustMatchSender:{else:'true',match:{'0':{if:`authenticated_as == '${login}'`,then:'false'}}}}}});
  compose('restart',a.service);await eventually(()=>a.client.call('x:MtaStageAuth/get',{ids:['singleton']}));
  await mailer.sendMail({from:`alice@${a.domain}`,to:`alice@${a.domain}`,subject:'Scoped submission service fixture',text:'Sent with authenticate and emailSend only.'});
  const client=new JmapClient({url:a.url,allowInsecureHttp:true,authorization:basic(`alice@${a.domain}`,a.password),accountId:accountIDs[0]});
  await eventually(async()=>assert.ok((await client.emails({text:'Scoped submission service fixture'})).list.length));
  const response=await fetch(a.url+'/jmap',{method:'POST',headers:{'Content-Type':'application/json',Authorization:basic(login,a.password)},body:JSON.stringify({using:['urn:ietf:params:jmap:core','urn:ietf:params:jmap:mail'],methodCalls:[['Email/query',{accountId:serviceID},'denied']]})});
  const result=await response.json();assert.equal(result.methodResponses[0][0],'error');assert.equal(result.methodResponses[0][1].type,'forbidden');
  const management=new StalwartClient({url:a.url,authorization:basic(login,a.password),allowInsecureHttp:true});await assert.rejects(management.call('x:Account/get',{ids:null}));
 }finally{
  mailer.close();const {id,...fields}=original;await a.client.call('x:MtaStageAuth/set',{update:{singleton:fields}});await a.client.call('x:Account/set',{destroy:[serviceID]});
 }
});


test('CE persisted private certificate keeps the submission pin stable across restarts',async()=>{
 const a=instances[0],directory=mkdtempSync(join(tmpdir(),'webdock-ce-cert-'));
 const settings=((await a.client.call('x:SystemSettings/get',{ids:['singleton']})).list as {defaultCertificateId?:string|null}[])[0];
 let certificateID:string|undefined;
 const fingerprint=async()=>{const socket=tlsConnect({host:a.ip,port:465,rejectUnauthorized:false});try{await once(socket,'secureConnect');return createHash('sha256').update(socket.getPeerCertificate().raw).digest('hex');}finally{socket.destroy();}};
 try{
  execFileSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-days','2','-subj',`/CN=mail.${a.domain}`,'-addext',`subjectAltName=DNS:mail.${a.domain}`,'-keyout',join(directory,'key.pem'),'-out',join(directory,'cert.pem')],{stdio:'ignore'});
  const certificate=readFileSync(join(directory,'cert.pem'),'utf8'),privateKey=readFileSync(join(directory,'key.pem'),'utf8');
  const expected=createHash('sha256').update(new X509Certificate(certificate).raw).digest('hex');
  const created=await a.client.call('x:Certificate/set',{create:{private:{certificate:{'@type':'Text',value:certificate},privateKey:{'@type':'Text',secret:privateKey}}}});
  certificateID=(created.created as {private:{id:string}}).private.id;
  await a.client.call('x:SystemSettings/set',{update:{singleton:{defaultCertificateId:certificateID}}});
  for(let restart=0;restart<2;restart++){
   compose('restart',a.service);await eventually(async()=>assert.equal(await fingerprint(),expected));
  }
 }finally{
  await a.client.call('x:SystemSettings/set',{update:{singleton:{defaultCertificateId:settings.defaultCertificateId??null}}});
  if(certificateID)await a.client.call('x:Certificate/set',{destroy:[certificateID]});
  rmSync(directory,{recursive:true,force:true});
 }
});

test('CE shared Webmail draft exports through restricted user credentials and files Sent after service SMTP',async()=>{
 const a=instances[0],domain=((await a.client.call('x:Domain/get',{ids:null})).list as {id:string;name:string}[]).find(row=>row.name===a.domain)!;
 const permissions=JSON.parse(execFileSync('python3',['-c',"import json,sys;sys.path.insert(0,'agents/kubernetes');from mail_configuration import MAIL_PERMISSIONS;print(json.dumps(MAIL_PERMISSIONS))"],{encoding:'utf8'})) as string[];
 const group=await a.client.call('x:Account/set',{create:{shared:{'@type':'Group',name:'shared-queue',domainId:domain.id,roles:{'@type':'Default'},permissions:{'@type':'Inherit'},quotas:{maxDiskQuota:10485760}}}});
 const groupID=(group.created as {shared:{id:string}}).shared.id;
 const principals=await a.client.call('x:Account/set',{create:{reader:{'@type':'User',name:'queue-reader',domainId:domain.id,roles:{'@type':'User'},permissions:{'@type':'Replace',enabledPermissions:Object.fromEntries(permissions.map(name=>[name,true])),disabledPermissions:{}},credentials:{'0':{'@type':'Password',secret:a.password}},memberGroupIds:{[groupID]:true},encryptionAtRest:{'@type':'Disabled'},quotas:{maxDiskQuota:1048576}},sender:{'@type':'User',name:'queue-submit',domainId:domain.id,roles:{'@type':'User'},permissions:{'@type':'Replace',enabledPermissions:{authenticate:true,emailSend:true},disabledPermissions:{}},credentials:{'0':{'@type':'Password',secret:a.password}},quotas:{maxDiskQuota:1048576}}}});
 const ids=Object.values(principals.created as Record<string,{id:string}>).map(row=>row.id);
 const original=((await a.client.call('x:MtaStageAuth/get',{ids:['singleton']})).list as Record<string,unknown>[])[0];
 const login=`queue-submit@${a.domain}`,address=`shared-queue@${a.domain}`;
 let proxy:ReturnType<typeof tcpServer>|undefined;
 try{
  await a.client.call('x:MtaStageAuth/set',{update:{singleton:{mustMatchSender:{else:'true',match:{'0':{if:`authenticated_as == '${login}'`,then:'false'}}}}}});
  compose('restart',a.service);await eventually(()=>a.client.call('x:Account/get',{ids:[groupID]}));
  const client=new JmapClient({url:a.url,allowInsecureHttp:true,authorization:basic(`queue-reader@${a.domain}`,a.password),accountId:groupID,sender:{email:address,name:'Shared support'}});
  const identity=(await client.identities())[0];assert.ok(identity);
  const draft=await client.createDraft({identityId:identity.id,to:[{email:`alice@${a.domain}`}],subject:'Shared native SMTP path',text:'A user-scoped group draft sent through the private service.'});
  const exported=await client.exportDraft(draft.id,identity.id),message=prepareMessage(exported.raw,exported.sender,exported.to);
  const socket=tlsConnect({host:a.ip,port:465,rejectUnauthorized:false});await once(socket,'secureConnect');const pin=createHash('sha256').update(socket.getPeerCertificate().raw).digest('hex');socket.destroy();
  proxy=tcpServer(connection=>{const upstream=tcpConnect({host:a.ip,port:465});connection.pipe(upstream);upstream.pipe(connection);connection.on('error',()=>upstream.destroy());upstream.on('error',()=>connection.destroy());});
  proxy.listen(0,'127.0.0.1');await once(proxy,'listening');
  await submitSMTP({host:'127.0.0.1',port:(proxy.address() as {port:number}).port,allowLoopback:true,certificateSha256:pin,username:login,password:a.password},message);
  await client.fileSent(draft.id);
  const sent=(await client.mailboxes()).list.find(row=>row.role==='sent');assert.ok(sent);
  assert.ok((await client.emails({mailboxId:sent.id})).list.some(row=>row.id===draft.id));
  const alice=new JmapClient({url:a.url,allowInsecureHttp:true,authorization:basic(`alice@${a.domain}`,a.password),accountId:accountIDs[0]});
  await eventually(async()=>assert.ok((await alice.emails({text:'Shared native SMTP path'})).list.length));
 }finally{
  if(proxy)await new Promise<void>(resolve=>proxy!.close(()=>resolve()));
  const {id,...fields}=original;await a.client.call('x:MtaStageAuth/set',{update:{singleton:fields}});await a.client.call('x:Account/set',{destroy:[...ids,groupID]});
 }
});
