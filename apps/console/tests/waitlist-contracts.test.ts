import {test} from 'node:test';
import assert from 'node:assert/strict';
import {waitlistSubmission,waitlistDecision,waitlistEntry} from '../src/waitlist-contracts';
await test('waitlist accepts normalized contact and explicit privacy acceptance only',()=>{
 assert.deepEqual(waitlistSubmission.parse({email:'  Test@Example.com  ',name:'  Test  ',privacyAccepted:true}),{email:'test@example.com',name:'Test',privacyAccepted:true});
 for(const input of [{email:'a@b.test',privacyAccepted:false},{email:'invalid',privacyAccepted:true},{email:'a@b.test',privacyAccepted:true,status:'approved'},{email:'a@b.test',name:'x'.repeat(101),privacyAccepted:true}])assert.equal(waitlistSubmission.safeParse(input).success,false);
});
await test('operator decisions carry the observed version and cannot promote an operator',()=>{
 assert.deepEqual(waitlistDecision.parse({action:'approve',version:2}),{action:'approve',version:2});
 for(const input of [{action:'operator',version:1},{action:'approve',version:0},{action:'approve',version:1,role:'operator'}])assert.equal(waitlistDecision.safeParse(input).success,false);
 assert.equal(waitlistEntry.safeParse({id:'10',email:'a@b.test',name:null,status:'pending',createdAt:'2026-10-31T00:00:00Z',reviewedAt:null,version:1}).success,true);
});

await test('a legacy malformed contact cannot poison the operator page decoder',()=>{
 const record={id:'11',email:'a..b@example.test',name:null,status:'pending',createdAt:'2026-10-31T00:00:00Z',reviewedAt:null,version:1};
 assert.equal(waitlistEntry.safeParse(record).success,true);
 assert.equal(waitlistSubmission.safeParse({email:record.email,privacyAccepted:true}).success,false);
});

await test('waitlist contact names reject embedded Unicode control characters',()=>{
 for(const control of ['\u0000','\u001f','\u007f','\u0080','\u009f'])assert.equal(waitlistSubmission.safeParse({email:'contact@example.test',name:`Contact${control}Name`,privacyAccepted:true}).success,false,`Accepted control U+${control.charCodeAt(0).toString(16)}`);
 assert.equal(waitlistSubmission.safeParse({email:'contact@example.test',name:'Zoë Müller 👋',privacyAccepted:true}).success,true);
});
