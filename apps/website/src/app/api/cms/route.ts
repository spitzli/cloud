import { handleCMSRequest } from '@spitzli/cms-ui/server';
import { getCMS,cmsOptions } from '@/lib/cms';
export const runtime='nodejs';
const handle=(request:Request)=>handleCMSRequest(request,cmsOptions,getCMS);
export { handle as GET,handle as POST,handle as DELETE };
