/** Owned Clerk DEVELOPMENT resources only. No production users or login bypass claims. */
import {readFileSync,writeFileSync} from 'node:fs';
import {parseEnv} from 'node:util';
import {randomBytes} from 'node:crypto';
import assert from 'node:assert/strict';
import {createClerkClient} from '@clerk/backend';
import {createClerkBackend} from '../../packages/auth-clerk/src/sdk';
import {resolveConsolePrincipal} from '../../legacy/control-plane/src/modules/auth/clerk-identity';
const path=process.argv[2];if(!path)throw Error('Dedicated development env file required');
const env=parseEnv(readFileSync(path,'utf8'));
const secret=env.CLERK_SECRET_KEY,pub=env.CLERK_PUBLISHABLE_KEY??env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY;
assert.ok(secret?.startsWith('sk_test_')&&pub?.startsWith('pk_test_'));
const issuer='https://'+Buffer.from(pub!.slice(8),'base64').toString().replace(/\$$/,'');
assert.equal(issuer,'https://on-sheep-3817.clerk.accounts.dev');
const client=createClerkClient({secretKey:secret,publishableKey:pub,telemetry:{disabled:true}});
const backend=createClerkBackend(secret!);
const owned='webdock-role-acceptance-'+Date.now()+'-'+randomBytes(5).toString('hex');
let userID:string|undefined,orgID:string|undefined,sessionID:string|undefined;
const proof:Record<string,unknown>={development:true,localIdentityMappingFixture:true,serverOwnedSessionReference:true,browserLoginTested:false,checks:[]};
const checked=(name:string)=>(proof.checks as string[]).push(name);
try{
 const user=await client.users.createUser({externalId:owned,emailAddress:[owned+'+clerk_test@example.com'],password:randomBytes(30).toString('base64url'),privateMetadata:{webdockRole:'user',fixture:owned},unsafeMetadata:{webdockRole:'operator'},publicMetadata:{webdockRole:'operator'}});
 userID=user.id;
 const org=await client.organizations.createOrganization({name:owned,createdBy:userID,privateMetadata:{fixture:owned}});orgID=org.id;
 const session=await client.sessions.createSession({userId:userID});sessionID=session.id;
 const local={id:'980000000000000001',name:'Role fixture',email:'fixture@example.test',role:'operator',banned:false,enabled:true,emailVerified:true};
 const config={issuer,authorizedParties:['https://console.example.test'],serviceKey:'fixture-service-key'.repeat(3)};
 const claims={iss:issuer,sub:userID,sid:sessionID,orgId:orgID,azp:config.authorizedParties[0],exp:Math.floor(Date.now()/1000)+3600};
 const dependencies={verify:async()=>claims,session:(id:string)=>client.sessions.getSession(id),user:(id:string)=>client.users.getUser(id),identity:async()=>local,organization:async()=>({id:'980000000000000002',customerID:'980000000000000003',active:true,enabled:true}),memberships:async(id:string)=>(await client.users.getOrganizationMembershipList({userId:id,limit:100})).data};
 const resolve=()=>resolveConsolePrincipal(config,dependencies,claims);
 assert.equal((await resolve()).operator,false);assert.equal((await backend.user(userID)).role,'user');checked('local operator, public metadata and unsafe metadata cannot grant');
 await client.users.updateUserMetadata(userID,{privateMetadata:{webdockRole:'operator'}});local.role='user';assert.equal((await resolve()).operator,true);assert.equal((await backend.user(userID)).role,'operator');checked('fresh private Clerk role grants despite local user role');
 await client.users.updateUserMetadata(userID,{privateMetadata:{webdockRole:'user'}});local.role='operator';assert.equal((await resolve()).operator,false);checked('same-session Clerk downgrade removes operator');
 await client.organizations.updateOrganizationMembership({organizationId:orgID,userId:userID,role:'org:member'});assert.equal((await resolve()).role,'member');checked('live organization role update');
 await client.organizations.deleteOrganizationMembership({organizationId:orgID,userId:userID});await assert.rejects(resolve);checked('membership removal denies workspace');
 await client.sessions.revokeSession(sessionID);await assert.rejects(resolve);checked('revoked provider session denied');
 proof.passed=true;
}finally{
 if(orgID){const org=await client.organizations.getOrganization({organizationId:orgID});assert.equal(org.privateMetadata.fixture,owned);await client.organizations.deleteOrganization(orgID);}
 if(userID){const user=await client.users.getUser(userID);assert.equal(user.externalId,owned);await client.users.deleteUser(userID);}
 proof.cleanedUp=true;
 writeFileSync('/tmp/webdock-clerk-role-live-proof.json',JSON.stringify(proof,null,2)+'\n',{mode:0o600});
}
console.log(JSON.stringify(proof));
