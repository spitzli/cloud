import {getRequestI18n} from '@spitzli/i18n/next';
import { headers } from 'next/headers';
import { redirect } from 'next/navigation';
import { cmsUser } from '@spitzli/cms-ui/server';
import { CMSApp } from '@spitzli/cms-ui/client';
import { getCMS,cmsOptions } from '@/lib/cms';
export const dynamic='force-dynamic';
export default async function Page(){const {locale,preference}=await getRequestI18n();const user=await cmsUser(await getCMS(),await headers());if(!user)redirect('/api/sso/login?returnTo=/cms');const issuer=process.env.SPITZLI_AUTH_ISSUER||'https://auth.webdock.dev/api/auth';return <CMSApp uiLocale={locale} uiPreference={preference} siteName={cmsOptions.siteName} siteURL={cmsOptions.siteURL} accountURL={new URL('/account',issuer).href} accessURL={new URL('/people',issuer).href} technicalURL="/system"/>;}
