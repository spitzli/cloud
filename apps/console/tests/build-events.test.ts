import assert from "node:assert/strict";
import {test} from "node:test";
import {readSse} from "../src/sse";
await test("SSE preserves fragmented UTF-8, cursor and multiline event data",async()=>{
  const bytes=new TextEncoder().encode(': keepalive\r\n\r\nid: 42\r\nevent: logs\r\ndata: {"text":"größer"}\r\n\r\n');
  const stream=new ReadableStream<Uint8Array>({start(c){for(const byte of bytes)c.enqueue(new Uint8Array([byte]));c.close();}});
  const events:unknown[]=[];await readSse(stream,event=>{events.push(event);},new AbortController().signal);
  assert.deepEqual(events,[{id:"42",event:"logs",data:'{"text":"größer"}'}]);
});
await test("oversized SSE frames and aborted consumers are bounded",async()=>{
  const stream=new ReadableStream<Uint8Array>({start(c){c.enqueue(new TextEncoder().encode('data: '+"x".repeat(1024)));c.close();}});
  await assert.rejects(readSse(stream,()=>undefined,new AbortController().signal,128));
  const abort=new AbortController();abort.abort();
  await assert.rejects(readSse(new ReadableStream(),()=>undefined,abort.signal));
});
