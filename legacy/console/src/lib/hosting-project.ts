import 'server-only';
import { cache } from 'react';
import type { HostingProjectContext } from '@webdock/hosting-contracts';
import { hostingPageCall } from './hosting-client';
export const getHostingProject = cache((projectID: string) => hostingPageCall<HostingProjectContext>(
  { action: 'projects.get', projectID }, `/hosting/projects/${encodeURIComponent(projectID)}`,
));
