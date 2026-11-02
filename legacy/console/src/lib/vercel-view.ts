import 'server-only';
import {studioCall} from './studio-client';
import type {VercelProjectView} from '@webdock/api-client/integrations';
export const projectVercelStatus=(_actor:unknown,id:string)=>studioCall<VercelProjectView>('integrations.projectStatus',[id]);
