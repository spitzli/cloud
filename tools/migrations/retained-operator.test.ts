import {test} from 'node:test';
import assert from 'node:assert/strict';
import {validateRetainedOperatorEvidence} from './retained-operator';

test('first existing operator adoption requires current exact external authority, never email matching alone',()=>{
 const target={subject:'99866772345196544',email:'owner@example.test',issuer:'https://clerk.example.test',externalSubject:'user_owned'};
 const valid={issuer:target.issuer,externalSubject:target.externalSubject,email:target.email,emailVerified:true,operator:true,banned:false,locked:false,checkedAt:Date.now()};
 assert.doesNotThrow(()=>validateRetainedOperatorEvidence(target,valid));
 for(const changed of [{externalSubject:'user_other'},{issuer:'https://foreign.test'},{email:'other@example.test'},{emailVerified:false},{operator:false},{banned:true},{locked:true},{checkedAt:Date.now()-600000}])
  assert.throws(()=>validateRetainedOperatorEvidence(target,{...valid,...changed}),/Current exact provider ownership/);
});
