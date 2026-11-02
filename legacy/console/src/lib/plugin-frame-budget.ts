/** Browser replay memory is bounded; durable write receipts remain the host's responsibility. */
export function createPluginFrameBudget() {
  const active=new Set<string>(), recent=new Set<string>();
  return {
    reserve(id:string,bytes:number) {
      if(!id||id.length>128||!Number.isSafeInteger(bytes)||bytes<1||bytes>32768||active.size>=4||active.has(id)||recent.has(id))return false;
      active.add(id);recent.add(id);
      if(recent.size>1000)recent.delete(recent.values().next().value!);
      return true;
    },
    release(id:string){active.delete(id);},
  };
}
