import test from 'node:test';
import assert from 'node:assert/strict';
import {parseSendRequest,compileSendRequest} from '../src/sending/request.ts';
test('native request canonicalizes recipients, keeps BCC private and disables remote attachments',async()=>{
 const request=parseSendRequest({from:'Sender <sender@example.test>',to:['A@EXAMPLE.TEST'],bcc:['hidden@example.test'],subject:'Hello',text:'Body',attachments:[{filename:'hello.txt',content:'aGk='}]});
 assert.deepEqual(request.to,['a@example.test']);
 const message=await compileSendRequest(request);
 assert.match(message.raw.toString(),/hello.txt/);assert.doesNotMatch(message.raw.toString(),/Bcc:|hidden@example.test/);
 assert.deepEqual(message.bcc,['hidden@example.test']);
 assert.throws(()=>parseSendRequest({...request,from:'bad\r\nBcc: recipient@example.test'}));
 assert.throws(()=>parseSendRequest({...request,attachments:[{path:'https://example.test/private'}]}));
 assert.throws(()=>parseSendRequest({...request,attachments:[{filename:'x',content:'!!!!'}]}));
});
