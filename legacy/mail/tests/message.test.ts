import test from 'node:test';
import assert from 'node:assert/strict';
import {prepareMessage,stampMessage} from '../src/sending/message.ts';

test('native MIME handoff replaces From, strips private/BCC/auth headers, and preserves the body',()=>{
 const body='Private body remains unchanged.\r\nBcc: this is body text\r\n';
 const result=prepareMessage(Buffer.from('From: Impostor <bad@example.test>\r\nBcc: hidden@example.test\r\nX-Webdock-Submission: forged\r\n folded-secret\r\nDKIM-Signature: forged\r\nAuthentication-Results: forged\r\nSubject: Test\r\n\r\n'+body),{email:'SUPPORT@bücher.example',name:'Änne König'},['Alice@EXAMPLE.test','alice@example.test']);
 const [headers,...tail]=result.raw.toString().split('\r\n\r\n');
 assert.match(headers,/From: =\?UTF-8\?B\?/);assert.match(headers,/<support@xn--bcher-kva.example>/);
 assert.doesNotMatch(headers,/Bcc:|X-Webdock|DKIM|Authentication|forged|Impostor/i);
 assert.equal(tail.join('\r\n\r\n'),body);assert.deepEqual(result.to,['alice@example.test']);assert.equal(result.from,'support@xn--bcher-kva.example');assert.match(result.hash,/^[a-f0-9]{64}$/);
 const stamped=stampMessage(result.raw,'123','x'.repeat(43));assert.match(stamped.toString(),/^X-Webdock-Submission: 123\.x{43}\r\n/);
});
test('MIME handoff refuses injected envelope fields and malformed header continuations',()=>{
 const good=Buffer.from('Subject: Test\r\n\r\nBody\r\n');
 assert.throws(()=>prepareMessage(good,{email:'sender@example.test\r\nBcc:evil@example.test'},['recipient@example.test']));
 assert.throws(()=>prepareMessage(Buffer.from(' bad continuation\r\n\r\nBody'),{email:'sender@example.test'},['recipient@example.test']));
 assert.throws(()=>stampMessage(good,'123\r\nX-Evil: yes','x'.repeat(43)));
});

test('a message with no retained headers keeps its exact body boundary',()=>{
 const result=prepareMessage(Buffer.from('From: original@example.test\r\n\r\nExact body'),{email:'sender@example.test'},['recipient@example.test']);
 assert.equal(result.raw.toString(),'From: <sender@example.test>\r\n\r\nExact body');
 assert.throws(()=>prepareMessage(Buffer.from('Subject: test\r\n\r\nBody'),{email:'sender@example.test'},['bad..address@example.test']));
});
