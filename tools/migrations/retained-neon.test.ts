import {test} from 'node:test';
import assert from 'node:assert/strict';
import {validateRetainedNeonEvidence} from './retained-neon';
const now=Date.now();
const target={canonicalId:'101712215249260544',email:'member@example.test',role:'user',issuer:'https://clerk.webdock.dev',externalSubject:'user_ExactRetained',providerOrigin:'https://ep-lucky-base-b9eaymxz.neonauth.c-7.eu-central-1.aws.neon.tech/neondb/auth',providerSubject:'2b28db90-addd-431e-82f1-de0657dbea6e',snapshotSha256:'a'.repeat(64)};
const provider={issuer:target.issuer,externalSubject:target.externalSubject,primaryEmail:target.email,emailVerified:true,banned:false,locked:false,mfaEnabled:false,passkeys:0,checkedAt:now};
await test('retained member import needs the exact fresh verified source identity, not an email match',()=>{
 assert.deepEqual(validateRetainedNeonEvidence(target,provider,now),target);
 for(const change of [{externalSubject:'user_Other'},{issuer:'https://other.example'},{emailVerified:false},{banned:true},{locked:true},{mfaEnabled:true},{passkeys:1},{primaryEmail:'other@example.test'},{checkedAt:now-300001},{checkedAt:now+30001}])assert.throws(()=>validateRetainedNeonEvidence(target,{...provider,...change},now));
});
await test('retained member import cannot change operators or send credentials to another provider',()=>{
 for(const change of [{role:'operator'},{canonicalId:101712215249260544},{providerSubject:'not-a-uuid'},{providerOrigin:'https://user:password@ep-any.neonauth.c-7.eu-central-1.aws.neon.tech/neondb/auth'},{providerOrigin:'https://ep-any.neonauth.example/neondb/auth'},{providerOrigin:target.providerOrigin+'?credential=x'}])assert.throws(()=>validateRetainedNeonEvidence({...target,...change},provider,now));
});
