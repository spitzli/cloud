import 'server-only';
import { cache } from 'react';
import type { HostingAppView } from '@webdock/hosting-contracts';
import { hostingPageCall } from './hosting-client';

export type HostingApplication = HostingAppView & {operator:boolean; canManage:boolean; canRequestLogs:boolean; subscriptionRevision:number};
export const getHostingApplication = cache((appID:string) => hostingPageCall<HostingApplication>({action:'apps.get',appID}, `/hosting/apps/${encodeURIComponent(appID)}`));
