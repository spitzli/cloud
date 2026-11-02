import assert from "node:assert/strict";
import test, { afterEach } from "node:test";
import { JmapClient, JmapError } from "../src/stalwart/jmap.ts";

// Each test swaps fetch for a fixture; the real one comes back afterwards.
const realFetch = globalThis.fetch;
afterEach(() => { globalThis.fetch = realFetch; });
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function mockFetch(handler: (url: any, init: RequestInit) => Promise<Response>) {
  globalThis.fetch = handler as unknown as typeof fetch;
}
const options = { url: "https://mail.example.com", authorization: "Basic fixture", accountId: "account-a" };
test("JMAP data access pins the account and rejects mismatched responses", async () => {
  const calls: Record<string, unknown>[] = [];
  mockFetch(async (_url: string, init: RequestInit) => {
    assert.equal(init.redirect, "error");
    const body = JSON.parse(String(init.body));
    const [method, args, tag] = body.methodCalls[0];
    calls.push(args);
    return Response.json({ methodResponses: [[method, { accountId: "account-b", list: [] }, tag]] });
  });
  const client = new JmapClient(options);
  await assert.rejects(client.mailboxes(), /account/);
  assert.equal(calls[0].accountId, "account-a");
});

test("message listing is bounded, uses explicit properties and cannot inject a method", async () => {
  const calls: unknown[] = [];
  mockFetch(async (_url: string, init: RequestInit) => {
    const [method, args, tag] = JSON.parse(String(init.body)).methodCalls[0];
    calls.push([method, args]);
    assert.equal(args.accountId, "account-a");
    const result = method === "Email/query" ? { ids: ["email1"], total: 1, queryState: "q1" } : { list: [{ id: "email1", subject: "Hello" }], state: "s1" };
    return Response.json({ methodResponses: [[method, { ...result, accountId: args.accountId }, tag]] });
  });
  const client = new JmapClient(options);
  await assert.rejects(client.emails({ limit: 101 }), /limit/);
  assert.equal(calls.length, 0);
  const result = await client.emails({ mailboxId: "inbox", text: "invoice", limit: 25 });
  assert.equal(result.list[0].subject, "Hello");
  assert.deepEqual((calls[0] as [string, { filter: unknown }])[1].filter, { inMailbox: "inbox", text: "invoice" });
});

test("a mutation is attempted once and uncertain outcomes remain explicit without exposing server details", async () => {
  let count = 0;
  mockFetch(async () => { count++; throw new Error("PRIVATE upstream detail"); });
  const client = new JmapClient(options);
  await assert.rejects(client.updateEmail("message1", { seen: true }), error => error instanceof JmapError && error.uncertain && !error.message.includes("PRIVATE"));
  assert.equal(count, 1);
});

test("foreign message IDs in an Email/get response are rejected and per-object errors are not success", async () => {
  mockFetch(async (_url: string, init: RequestInit) => {
    const [method, args, tag] = JSON.parse(String(init.body)).methodCalls[0];
    return Response.json({ methodResponses: [[method, { accountId: args.accountId,
      ...(method === "Email/get" ? { list: [{ id: "foreign" }] } : { notUpdated: { message1: { type: "forbidden", description: "private" } } }) }, tag]] });
  });
  const client = new JmapClient(options);
  await assert.rejects(client.email("message1"), /response/);
  await assert.rejects(client.updateEmail("message1", { seen: true }), error => error instanceof JmapError && !error.uncertain && !error.message.includes("private"));
});

test("drafts derive From from an allowed identity and submissions reject a forged draft sender", async () => {
  let supplied: Record<string, unknown> | undefined;
  mockFetch(async (_url: string, init: RequestInit) => {
    const [method, args, tag] = JSON.parse(String(init.body)).methodCalls[0];
    let result: Record<string, unknown>;
    if (method === "Identity/get") result = { list: [{ id: "sender", email: "alice@example.com", name: "Alice" }] };
    else if (method === "Mailbox/get") result = { list: [{ id: "drafts", role: "drafts" }] };
    else if (method === "Email/get") result = { list: [{ id: "draft1", from: [{ email: "forged@example.com" }], keywords: { $draft: true } }] };
    else { assert.equal(method, "Email/set"); supplied = args.create.draft; result = { created: { draft: { id: "draft1" } } }; }
    return Response.json({ methodResponses: [[method, { ...result, accountId: args.accountId }, tag]] });
  });
  const client = new JmapClient({ ...options, sender: { email: "alice@example.com", name: "Support" } });
  await assert.rejects(client.createDraft({ identityId: "foreign", to: [{ email: "recipient@example.com" }], subject: "Hi", text: "Body" }), /identity/);
  const draft = await client.createDraft({ identityId: "sender", to: [{ email: "recipient@example.com" }], subject: "Hi", text: "Body" });
  assert.equal(draft.id, "draft1");
  assert.deepEqual(supplied?.from, [{ email: "alice@example.com", name: "Support" }]);
  assert.deepEqual(supplied?.keywords, { $draft: true });
  await assert.rejects(client.submitDraft("draft1", "sender"), /sender/);
});

test("malformed submission receipts stay uncertain and must never permit an automatic resend", async () => {
  mockFetch(async (_url: string, init: RequestInit) => {
    const [method, args, tag] = JSON.parse(String(init.body)).methodCalls[0];
    const result = method === "Identity/get" ? { list: [{ id: "sender", email: "alice@example.com" }] }
      : method === "Email/get" ? { list: [{ id: "draft1", from: [{ email: "alice@example.com" }], keywords: { $draft: true } }] }
      : { created: { submission: { id: "invalid/id" } } };
    return Response.json({ methodResponses: [[method, { ...result, accountId: args.accountId }, tag]] });
  });
  await assert.rejects(new JmapClient(options).submitDraft("draft1", "sender"), error => error instanceof JmapError && error.uncertain);
});

test('sent filing clears draft and moves to the sent folder without another submission', async () => {
  const calls: string[]=[];
  mockFetch(async (_url:string,init:RequestInit)=>{
    const [method,args,tag]=JSON.parse(String(init.body)).methodCalls[0];calls.push(method);
    const result=method==='Mailbox/get'?{list:[{id:'sent',role:'sent'}]}:{updated:{draft1:null}};
    if(method==='Email/set')assert.deepEqual(args.update.draft1,{'keywords/$draft':null,mailboxIds:{sent:true}});
    return Response.json({methodResponses:[[method,{...result,accountId:args.accountId},tag]]});
  });
  await new JmapClient(options).fileSent('draft1');
  assert.deepEqual(calls,['Mailbox/get','Email/set']);
});

test('attachments use fixed account paths, bounded uploads and message-owned download IDs', async () =>{
  const paths:string[]=[];
  mockFetch(async (url:URL,init:RequestInit)=>{
    paths.push(String(url));
    if(String(url).includes('/jmap/upload/'))return Response.json({accountId:'account-a',blobId:'blob1',type:'text/plain',size:5});
    if(String(url).includes('/jmap/download/'))return new Response('hello');
    const [method,args,tag]=JSON.parse(String(init.body)).methodCalls[0];
    return Response.json({methodResponses:[[method,{accountId:args.accountId,list:[{id:'email1',attachments:[{blobId:'blob1',name:'note.txt',type:'text/plain',size:5}]}]},tag]]});
  });
  const client=new JmapClient(options);
  assert.equal((await client.upload(Buffer.from('hello'),'text/plain')).blobId,'blob1');
  await assert.rejects(client.downloadAttachment('email1','foreign'),/attachment/i);
  const file=await client.downloadAttachment('email1','blob1');assert.equal(file.bytes.toString(),'hello');
  assert.ok(paths.some(path=>path.includes('/jmap/upload/account-a/')));
  assert.ok(paths.some(path=>path.includes('/jmap/download/account-a/blob1/')));
  assert.ok(!paths.some(path=>path.includes('/foreign/')));
});

test('draft replacement checks the current draft and uses an optimistic JMAP state fence',async () =>{
  const changes:Record<string,unknown>[]=[];
  mockFetch(async (_url:string,init:RequestInit)=>{
    const [method,args,tag]=JSON.parse(String(init.body)).methodCalls[0];
    const result=method==='Identity/get'?{list:[{id:'sender',email:'alice@example.com'}]}:method==='Mailbox/get'?{list:[{id:'drafts',role:'drafts'}]}:method==='Email/get'?{state:'version1',list:[{id:'old-draft',from:[{email:'alice@example.com'}],keywords:{$draft:true}}]}:(changes.push(args),args.destroy?{destroyed:['old-draft'],newState:'version3'}:{created:{draft:{id:'new-draft'}},newState:'version2'});
    return Response.json({methodResponses:[[method,{...result,accountId:args.accountId},tag]]});
  });
  const result=await new JmapClient(options).createDraft({identityId:'sender',to:[],subject:'Working draft',text:'Edited'},'old-draft');
  assert.equal(result.id,'new-draft');assert.equal(changes[0].destroy,undefined);assert.equal(changes[0].ifInState,'version1');assert.deepEqual(changes[1].destroy,['old-draft']);assert.equal(changes[1].ifInState,'version2');
});

test('a failed replacement creation never deletes the last saved draft',async () =>{
  let destroyed=false;
  mockFetch(async (_url:string,init:RequestInit)=>{
    const [method,args,tag]=JSON.parse(String(init.body)).methodCalls[0];
    if(args.destroy)destroyed=true;
    const result=method==='Identity/get'?{list:[{id:'sender',email:'alice@example.com'}]}:method==='Mailbox/get'?{list:[{id:'drafts',role:'drafts'}]}:method==='Email/get'?{state:'version1',list:[{id:'old-draft',from:[{email:'alice@example.com'}],keywords:{$draft:true}}]}:{notCreated:{draft:{type:'overQuota'}},...(args.destroy?{destroyed:args.destroy}:{})};
    return Response.json({methodResponses:[[method,{...result,accountId:args.accountId},tag]]});
  });
  await assert.rejects(new JmapClient(options).createDraft({identityId:'sender',to:[],subject:'Update',text:'Must retain old content'},'old-draft'));
  assert.equal(destroyed,false);
});

test('SMTP draft export uses the selected account, validates From and retains BCC only in the envelope data',async () =>{
 mockFetch(async (url:URL,init:RequestInit)=>{
  if(String(url).includes('/jmap/download/')){assert.match(String(url),/\/account-a\/blob1\//);return new Response('From: alice@example.com\r\nBcc: hidden@example.com\r\n\r\nBody');}
  const [method,args,tag]=JSON.parse(String(init.body)).methodCalls[0];
  const result=method==='Identity/get'?{list:[{id:'sender',email:'alice@example.com'}]}:{list:[{id:'draft1',blobId:'blob1',size:100,keywords:{$draft:true},from:[{email:'alice@example.com'}],to:[{email:'bob@example.com',name:null}],bcc:[{email:'hidden@example.com',name:null}]}]};
  return Response.json({methodResponses:[[method,{...result,accountId:args.accountId},tag]]});
 });
 const message=await new JmapClient(options).exportDraft('draft1','sender');
 assert.deepEqual(message.to,['bob@example.com','hidden@example.com']);assert.equal(message.sender.email,'alice@example.com');assert.ok(message.raw.includes('Body'));
});
