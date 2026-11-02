import { getRequestI18n } from '@webdock/i18n/next';
import { getHostingApplication } from '@/lib/hosting-application';
import { HostingProjectHeader } from '@/components/hosting/project-context';
import { applicationState } from '@/lib/workspace-navigation';
import { HostingRefresh } from '@/components/hosting/refresh';
import Link from 'next/link';

export default async function ApplicationLayout({children,params}: {children:React.ReactNode;params:Promise<{appID:string}>}) {
  const { appID } = await params, { t } = await getRequestI18n();
  const app = await getHostingApplication(appID), base=`/hosting/apps/${appID}`;
  return <>
    <HostingProjectHeader projectID={app.projectID} leaf={{label:app.name,href:base}} showNavigation={false}/>
    <div className="page-heading"><div><h1>{app.name}</h1><p>{t('Observed state')}: {t(applicationState(app.status))}</p><small>{t('Applied revision')}: {app.observedRevision} / {app.revision}</small></div><Link href={`/hosting/projects/${app.projectID}`}>{t('Applications')}</Link></div>
    <HostingRefresh active={app.status==='pending'}/>
    {app.lastError && <p role="alert" className="notice error">{t(app.lastError)}</p>}
    {app.operationID && <p><Link href={`/hosting/operations/${app.operationID}`}>{t('View latest operation')}</Link></p>}
    {children}
  </>;
}
