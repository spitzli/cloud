import {getRequestI18n} from '@spitzli/i18n/next';
import {localizeLanding} from './landing-copy';
import { cache } from 'react';
import { getPayload } from 'payload';
import config from '@payload-config';
export const getLanding = cache(async () => {
 const payload=await getPayload({config});
 const page=await payload.findGlobal({slug:'landing-page',depth:0,overrideAccess:true});
 const {t}=await getRequestI18n();
 return localizeLanding(page,t);
});
