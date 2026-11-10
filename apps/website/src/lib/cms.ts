import {msgid} from '@spitzli/i18n';
import { getPayload } from 'payload';
import config from '@payload-config';
import { headers } from 'next/headers';
import { redirect } from 'next/navigation';
import { cmsUser } from '@spitzli/cms-ui/server';
import type { CMSOptions } from '@spitzli/cms-ui/types';
export const cmsOptions: CMSOptions={siteName:'Spitzli Cloud',siteURL:process.env.NEXT_PUBLIC_SERVER_URL||'https://spitzli.cloud',modules:[{slug:'landing-page',kind:'global',label:msgid("Website content"),description:msgid("Edit your homepage, contact details and search appearance."),previewPath:'/'}]};
export const getCMS=()=>getPayload({config});
export async function requireCMSOperator(){const p=await getCMS();const user=await cmsUser(p,await headers());if(!user)redirect('/api/sso/login?returnTo=/system');if(user.role!=='operator')redirect('/cms');return user;}
