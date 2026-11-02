import { redirect } from "next/navigation";
export default async function Hosting({params,searchParams}: {params:Promise<{id:string}>;searchParams:Promise<Record<string,string|string[]|undefined>>}) {
  const {id}=await params, values=await searchParams, query=new URLSearchParams();
  for(const key of ["q","page","sort"]) { const value=values[key]; if(typeof value==='string') query.set(key,value); }
  redirect(`/tenants/${encodeURIComponent(id)}/hosting${query.size?'?'+query:''}`);
}
