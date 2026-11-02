import test from "node:test";
import assert from "node:assert/strict";
import { NextRequest } from "next/server";
import { createClerkRequest } from "@clerk/backend/internal";
import { canonicalClerkRequest } from "../src/lib/clerk-request-origin";

test("Clerk handshake uses configured public origin and preserves path, cookies and POST body", async () => {
  const request = new NextRequest("https://deployment.vercel.app/login?returnTo=%2Ftenants", {
    method: "POST", body: "action=example",
    headers: {"x-forwarded-host": "deployment.vercel.app", "x-forwarded-proto": "https", cookie: "fixture=value", origin: "https://untrusted.example"},
  });
  const normalized = canonicalClerkRequest(request, "https://console.webdock.dev");
  assert.equal(createClerkRequest(normalized).clerkUrl.href, "https://console.webdock.dev/login?returnTo=%2Ftenants");
  assert.equal(normalized.nextUrl.origin, "https://console.webdock.dev");
  assert.equal(normalized.headers.get("origin"), "https://untrusted.example", "preserve original Origin for CSRF checks");
  assert.equal(normalized.cookies.get("fixture")?.value, "value");
  assert.equal(await normalized.text(), "action=example");
});

test("request-supplied forwarded origins cannot choose the handshake destination", () => {
  const request = new NextRequest("https://deployment.vercel.app/", {headers: {"x-forwarded-host": "attacker.example", "x-forwarded-proto": "http"}});
  assert.equal(createClerkRequest(canonicalClerkRequest(request, "https://console.webdock.dev")).clerkUrl.origin, "https://console.webdock.dev");
  assert.throws(() => canonicalClerkRequest(request, "http://console.webdock.dev"));
});

test("network-path references cannot escape the configured origin", () => {
  const request = new NextRequest("https://deployment.vercel.app//attacker.example/login");
  assert.throws(() => canonicalClerkRequest(request, "https://console.webdock.dev"), /Ambiguous/);
});

test("explicit development loopback origin replaces the internal Next listener", () => {
 const request=new NextRequest('http://127.0.0.1:3470/login?returnTo=%2Fprojects');
 const normalized=canonicalClerkRequest(request,'http://localhost:3460',{allowLocalHTTP:true,environment:'development'});
 assert.equal(createClerkRequest(normalized).clerkUrl.href,'http://localhost:3460/login?returnTo=%2Fprojects');
 assert.equal(normalized.url,'http://localhost:3460/login?returnTo=%2Fprojects');
 assert.throws(()=>canonicalClerkRequest(request,'http://localhost:3460',{allowLocalHTTP:true,environment:'production'}));
 assert.throws(()=>canonicalClerkRequest(request,'http://evil.example',{allowLocalHTTP:true,environment:'development'}));
});

test('Clerk same-URL auth decoration continues internally without losing verified headers', async () => {
 const {resumeClerkRequest}=await import('../src/lib/clerk-request-origin');
 const response=new Response(null,{headers:{'x-middleware-rewrite':'http://localhost:3460/login?next=1','x-middleware-override-headers':'x-clerk-auth-signature','x-middleware-request-x-clerk-auth-signature':'verified'}});
 resumeClerkRequest('http://localhost:3460/login?next=1',response);
 assert.equal(response.headers.get('x-middleware-rewrite'),null);
 assert.equal(response.headers.get('x-middleware-next'),'1');
 assert.equal(response.headers.get('x-middleware-request-x-clerk-auth-signature'),'verified');
 for(const target of ['https://elsewhere.example/login','http://localhost:3460/other']){
  const different=new Response(null,{headers:{'x-middleware-rewrite':target}});resumeClerkRequest('http://localhost:3460/login',different);assert.equal(different.headers.get('x-middleware-rewrite'),target);
 }
});
