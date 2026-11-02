'use server';
import {revalidatePath} from 'next/cache';
import {studioCall} from './studio-client';
export type VercelLinkState={error?:string;message?:string};
export async function linkVercelProject(_state:VercelLinkState,form:FormData):Promise<VercelLinkState>{
 try{const project=String(form.get('project')||''),vercelProject=String(form.get('vercelProject')||'');const result=await studioCall<VercelLinkState>('integrations.linkVercel',[{project,vercelProject}]);revalidatePath('/projects/'+project);revalidatePath('/integrations');return result;}
 catch{return{error:'The project could not be linked. Check access to both projects, then try again.'};}
}
