export interface SseEvent {id:string;event:string;data:string}

function parseFrame(frame:string,maxFrameBytes:number):SseEvent|undefined {
  if(new TextEncoder().encode(frame).byteLength>maxFrameBytes)throw new Error("Build log event exceeds its supported size.");
  let id="",event="message";const data:string[]=[];
  for(const line of frame.split("\n")){
    if(line.startsWith(":"))continue;
    const colon=line.indexOf(":"),key=colon<0?line:line.slice(0,colon);
    const text=colon<0?"":line.slice(colon+1).replace(/^ /,"");
    switch(key){
      case "id":
        if(text.includes("\0")||text.length>128)throw new Error("Invalid build log cursor.");
        id=text;break;
      case "event":event=text;break;
      case "data":data.push(text);break;
    }
  }
  if(data.length)return {id,event,data:data.join("\n")};
  return undefined;
}

async function emitFrame(frame:string,maxFrameBytes:number,onEvent:(event:SseEvent)=>void|Promise<void>,signal:AbortSignal):Promise<void> {
  const event=parseFrame(frame,maxFrameBytes);
  if(!event)return;
  signal.throwIfAborted();await onEvent(event);
}

/** Incremental UTF-8 with bounded frames; the consumer owns schema and cursor validation. */
export async function readSse(stream:ReadableStream<Uint8Array>,onEvent:(event:SseEvent)=>void|Promise<void>,signal:AbortSignal,maxFrameBytes=2*1024*1024):Promise<void> {
  signal.throwIfAborted();
  const reader=stream.getReader(),decoder=new TextDecoder("utf-8",{fatal:true});
  const abort=()=>{void reader.cancel().catch(()=>undefined);};
  signal.addEventListener("abort",abort,{once:true});
  let buffer="",finished=false;
  try {
    while(!finished){
      signal.throwIfAborted();
      const {value,done}=await reader.read();
      signal.throwIfAborted();
      finished=done;
      buffer+=done?decoder.decode():decoder.decode(value,{stream:true});
      buffer=buffer.replaceAll("\r\n","\n");
      let end:number;
      while((end=buffer.indexOf("\n\n"))>=0){
        const frame=buffer.slice(0,end);buffer=buffer.slice(end+2);
        await emitFrame(frame,maxFrameBytes,onEvent,signal);
      }
      if(new TextEncoder().encode(buffer).byteLength>maxFrameBytes)throw new Error("Build log event exceeds its supported size.");
    }
  }finally{signal.removeEventListener("abort",abort);await reader.cancel().catch(()=>undefined);reader.releaseLock();}
}
