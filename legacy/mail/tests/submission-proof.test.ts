import test from 'node:test';
import assert from 'node:assert/strict';
import {randomBytes} from 'node:crypto';
import {messageDigest,hookMessageDigest,signSubmission,verifySubmission} from '@webdock/mail-core/submission-proof';
test('SMTP admission proof binds content, customer, envelope and route revision',()=>{
 const raw=Buffer.from('From: a@example.test\r\nSubject: A\r\n folded subject\r\n\r\nBody\r\n');
 const digest=messageDigest(raw);
 assert.equal(digest,hookMessageDigest({headers:[['From',' a@example.test\r\n'],['Subject',' A\r\n folded subject\r\n'],['X-Webdock-Submission','123.proof']],contents:'Body\r\n',size:raw.length}));
 const key=randomBytes(32).toString('base64');
 const context={customerID:'123',id:'456',digest,from:'a@example.test',to:['b@example.test'],transportRevision:'2',serviceRevision:'3'};
 const proof=signSubmission(context,key);assert.equal(verifySubmission(context,proof,key),true);
 for(const altered of [{...context,customerID:'124'},{...context,digest:messageDigest(Buffer.from(raw.toString().replace('Body','Changed')))},{...context,to:['other@example.test']},{...context,serviceRevision:'4'}])assert.equal(verifySubmission(altered,proof,key),false);
 assert.equal(verifySubmission(context,'x'.repeat(43),key),false);
 assert.throws(()=>messageDigest(Buffer.from([255,13,10,13,10])));
});
