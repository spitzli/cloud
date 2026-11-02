import { test, expect } from "bun:test";
import { fileURLToPath } from "node:url";

test("OAuth portal lookup binds freshly verified Clerk authority to its introspected session", () => {
  const result = Bun.spawnSync([process.execPath,"--no-env-file","-e",`
    import { mock } from "bun:test";
    import assert from "node:assert/strict";
    import { currentDatabaseScope } from "@webdock/database-runtime";
    let proof={operator:true,twoFactorEnabled:true,issuer:"https://identity.example",userID:"provider",verifiedAt:Date.now(),expiresAt:Date.now()+59000,roles:[]};
    let queries=0;
    mock.module("@webdock/oauth-session",()=>({configureOAuthSession:options=>options}));
    mock.module("./apps/control-plane/src/modules/auth/introspection",()=>({introspectToken:async()=>{throw Error("unexpected");}}));
    mock.module("./apps/control-plane/src/modules/auth/clerk-provider",()=>({legacyClerkMemberships:async()=>proof}));
    mock.module("./apps/control-plane/src/db/runtime",()=>({database:{query:async()=>{queries++;assert.equal(currentDatabaseScope().memberships.operator,true);assert.equal(currentDatabaseScope().memberships.subject,"local");assert.equal(currentDatabaseScope().memberships.sessionID,"session");return {rows:[{value:{id:"registry"}}]};}}}));
    const {createPortalSession}=await import("./apps/control-plane/src/modules/portal/session");
    const lookup=createPortalSession().lookupUser;
    assert.deepEqual(await lookup({sub:"local",sessionID:"session",role:"operator"}),{id:"registry"});
    proof.operator=false;
    assert.equal(await lookup({sub:"local",sessionID:"session",role:"operator"}),null);
    proof=undefined;
    assert.equal(await lookup({sub:"local",sessionID:"session",role:"operator"}),null);
    assert.equal(queries,1);
  `],{cwd:fileURLToPath(new URL("../../..",import.meta.url)),env:{...process.env,DATABASE_RLS_SECRET:"fixture-key-32-characters-long-test",WEBDOCK_AUTH_PROVIDER:JSON.stringify({provider:"clerk",issuer:"https://identity.example",authorizedParties:["https://console.example"],audience:"console"})},stdout:"pipe",stderr:"pipe"});
  expect(result.exitCode,new TextDecoder().decode(result.stderr)).toBe(0);
});

test("authenticated database wrapper supplies fresh proof for a Clerk compatibility business identity", () => {
  const result = Bun.spawnSync([process.execPath,"--no-env-file","-e",`
    import { mock } from "bun:test";
    import assert from "node:assert/strict";
    import { currentDatabaseScope } from "@webdock/database-runtime";
    const proof={operator:false,twoFactorEnabled:false,issuer:"https://identity.example",userID:"external",verifiedAt:Date.now(),expiresAt:Date.now()+59000,roles:[]};
    let proofReads=0;
    mock.module("./apps/control-plane/src/modules/auth/auth",()=>({auth:{api:{getSession:async()=>({user:{id:"local",role:"operator"},session:{id:"session"}})}}}));
    mock.module("./apps/control-plane/src/modules/auth/principal",()=>({currentConsolePrincipal:()=>null,resolveSessionReference:async()=>({subject:"local",sessionID:"session"}),selectedIdentityDirectory:{session:async()=>({role:"user",emailVerified:true,banned:false,twoFactorEnabled:false,mustChangePassword:false})}}));
    mock.module("./apps/control-plane/src/modules/auth/mcp",()=>({currentMCPClaims:async()=>({disabled:true})}));
    mock.module("./apps/control-plane/src/modules/auth/clerk-provider",()=>({legacyClerkMemberships:async()=>{proofReads++;return proof;}}));
    mock.module("./apps/control-plane/src/db/runtime",()=>({authDatabase:{}}));
    const {withAuthenticatedDatabase}=await import("./apps/control-plane/src/db/database-context");
    await withAuthenticatedDatabase(new Headers(),async()=>{
      assert.equal(currentDatabaseScope().memberships?.operator,false);
      assert.equal(currentDatabaseScope().memberships?.subject,"local");
      assert.equal(currentDatabaseScope().memberships?.sessionID,"session");
      assert.equal(currentDatabaseScope().authProvider,"clerk");
    });
    assert.equal(proofReads,1);
  `],{cwd:fileURLToPath(new URL("../../..",import.meta.url)),env:{...process.env,WEBDOCK_AUTH_PROVIDER:JSON.stringify({provider:"clerk",issuer:"https://identity.example",authorizedParties:["https://console.example"],audience:"console"})},stdout:"pipe",stderr:"pipe"});
  expect(result.exitCode,new TextDecoder().decode(result.stderr)).toBe(0);
});
