import 'server-only';
import {studioCall} from './studio-client';
import type {WebsiteContentField,ContentVariant} from '@webdock/api-client/cms';
export type {WebsiteContentField,ContentVariant} from '@webdock/api-client/cms';
export const websiteContent=(id:string,variant:ContentVariant)=>studioCall<{docs:WebsiteContentField[]}>('cms.content',[id,variant]);
export const saveWebsiteContent=(id:string,variant:ContentVariant,body:{key:string;value:string;updatedAt:string})=>studioCall<{doc:WebsiteContentField}>('cms.saveContent',[id,variant,body]);
