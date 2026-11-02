import { test, expect } from "bun:test";
import { fileURLToPath } from "node:url";

test("OAuth client administration uses fresh provider role and MFA when Clerk is selected", () => {
  const result = Bun.spawnSync([process.execPath,"--no-env-file","-e",`
    import { mock } from "bun:test";
    import assert from "node:assert/strict";
    let oauth, proof={operator:false,twoFactorEnabled:true};
    mock.module("@webdock/auth-better-auth/runtime",()=>({createBetterAuthRuntime: options=>options()}));
    mock.module("@better-auth/oauth-provider",()=>({oauthProvider: options=>{oauth=options;return {id:"oauth-provider"};}}));
    mock.module("./apps/control-plane/src/db/runtime",()=>({authDatabase:{query:async()=>({rows:[]})},database:{query:async()=>({rows:[]})}}));
    mock.module("./apps/control-plane/src/modules/auth/clerk-provider",()=>({legacyClerkMemberships:async()=>proof,linkedMembershipRole:async()=>null}));
    await import("./apps/control-plane/src/modules/auth/auth");
    const user={id:"local",role:"operator",twoFactorEnabled:true,mustChangePassword:false};
    assert.equal(await oauth.clientPrivileges({user,action:"update"}),false);
    user.twoFactorEnabled=false;
    assert.equal(await oauth.postLogin.shouldRedirect({user}),false);
    proof={operator:true,twoFactorEnabled:true};user.role="user";user.twoFactorEnabled=false;
    assert.equal(await oauth.clientPrivileges({user,action:"update"}),true);
    proof.operator=false;
    assert.equal(await oauth.clientPrivileges({user,action:"update"}),false);
    proof={operator:true,twoFactorEnabled:false};
    assert.equal(await oauth.postLogin.shouldRedirect({user}),true);
    assert.equal(await oauth.clientPrivileges({user,action:"update"}),false);
    proof=undefined;
    assert.equal(await oauth.clientPrivileges({user,action:"update"}),false);
    delete process.env.WEBDOCK_AUTH_PROVIDER;
    user.role="operator";user.twoFactorEnabled=true;
    assert.equal(await oauth.clientPrivileges({user,action:"update"}),true);
    assert.equal(await oauth.clientPrivileges({user,action:"create"}),false);
  `],{cwd:fileURLToPath(new URL("../../..",import.meta.url)),env:{...process.env,WEBDOCK_AUTH_PROVIDER:JSON.stringify({provider:"clerk",issuer:"https://identity.example",authorizedParties:["https://console.example"],audience:"console"})},stdout:"pipe",stderr:"pipe"});
  expect(result.exitCode,new TextDecoder().decode(result.stderr)).toBe(0);
});
