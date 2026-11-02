import 'server-only';
import {AccessError} from './studio-errors';
import {getStudioSession} from './studio-client';
export const mcpResource=process.env.WEBDOCK_MCP_RESOURCE||'https://studio.webdock.dev/api/mcp';
export async function currentMCPClaims(subject:string|undefined){
 try{const session=await getStudioSession();return{disabled:!subject||session.user.id!==subject||!session.operator};}
 catch(error){if(error instanceof AccessError&&[401,403].includes(error.status))return{disabled:true};throw error;}
}
