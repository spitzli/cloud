import {createPreferenceResponse} from '@spitzli/i18n/preference';
export const POST=(request:Request)=>createPreferenceResponse(request,{canonicalOrigin:new URL(process.env.NEXT_PUBLIC_SERVER_URL||'https://webdock.dev').origin});
