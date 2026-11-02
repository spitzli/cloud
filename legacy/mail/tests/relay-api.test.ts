import test from 'node:test';
import assert from 'node:assert/strict';
import {submitRelayAPI,RelayAPIFailure} from '../src/sending/relay-api.ts';
const account={region:'eu' as const,consumerKey:'fixture-key',consumerSecret:'fixture-secret'};
const message={referenceID:'123',sender:{email:'sender@example.test'},to:['to@example.test'],cc:['cc@example.test'],bcc:['blind@example.test'],subject:'Fixture',raw:Buffer.from('From: forged@example.test\r\nBcc: blind@example.test\r\nSubject: Fixture\r\n\r\nHello')};
test('direct relay uses the assigned region, preserves BCC and exact 64-bit acceptance ID',async()=>{
 let calls=0;
 const result=await submitRelayAPI(account,message,async(url,init)=>{
  calls++; assert.equal(url,'https://api.eu.turbo-smtp.com/api/v2/mail/send');
  assert.equal(init?.redirect,'error');assert.equal(new Headers(init?.headers).get('consumerSecret'),'fixture-secret');
  const body=JSON.parse(String(init?.body));
  assert.equal(body.reference_id,'123');assert.equal(body.to,'to@example.test');assert.equal(body.cc,'cc@example.test');assert.equal(body.bcc,'blind@example.test');
  assert.match(body.mime_raw,/From: <sender@example.test>/);assert.doesNotMatch(body.mime_raw,/Bcc:|forged/);
  return new Response('{"message":"success","mid":9223372036854775807}');
 });
 assert.deepEqual(result,{providerID:'9223372036854775807'});assert.equal(calls,1);
});
test('direct relay never retries uncertain handoffs and never exposes upstream response text',async()=>{
 for(const response of [()=>{throw Error('fixture-secret');},()=>new Response('fixture-secret',{status:503}),()=>new Response('{}'),()=>new Response('{"mid":1.5}'),()=>new Response('x'.repeat(65537))]){
  let calls=0;
  await assert.rejects(submitRelayAPI(account,message,async()=>{calls++;return response();}),error=>error instanceof RelayAPIFailure && error.uncertain && !error.message.includes('fixture-secret'));
  assert.equal(calls,1);
 }
 for(const status of [400,401,403,413,429])await assert.rejects(submitRelayAPI(account,message,async()=>new Response('fixture-secret',{status})),error=>error instanceof RelayAPIFailure && !error.uncertain);
});
test('direct relay validates before sending; only fixed regional endpoints are available',async()=>{
 let calls=0;const send:typeof fetch=async()=>{calls++;return new Response('{"mid":1}');};
 for(const invalid of [{...message,to:['bad\r\n@example.test']},{...message,subject:'bad\r\nheader'},{...message,referenceID:'../secret'},{...message,raw:Buffer.from([255])}])await assert.rejects(submitRelayAPI(account,invalid,send));
 await assert.rejects(submitRelayAPI({...account,region:'other' as 'eu'},message,send));assert.equal(calls,0);
 await submitRelayAPI({...account,region:'global'},message,async(url)=>{assert.equal(url,'https://api.turbo-smtp.com/api/v2/mail/send');return new Response('{"mid":"123"}');});
});
