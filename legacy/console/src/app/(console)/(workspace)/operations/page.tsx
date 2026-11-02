import Link from 'next/link';
import { getRequestI18n } from '@webdock/i18n/next';
import type { HostingOverview } from '@webdock/hosting-contracts';
import { hostingCall } from '@/lib/hosting-client';
import { applicationState } from '@/lib/workspace-navigation';
import { uiLabel } from '@/lib/ui-labels';
import { HostingTable } from '@/components/hosting/table';
import { ListControls, Pagination } from '@/components/list-controls';
import { HostingRefresh } from '@/components/hosting/refresh';

export default async function Operations({searchParams}: {searchParams:Promise<{q?:string;page?:string;sort?:string}>}) {
  const { t, date } = await getRequestI18n(), query = await searchParams;
  const q = (query.q ?? '').slice(0,160), page = /^[1-9][0-9]{0,3}$/.test(query.page??'') ? Number(query.page) : 1, sort = query.sort === '-name' ? '-name' : 'name';
  let data: HostingOverview | undefined;
  try { data = await hostingCall<HostingOverview>({action:'operations.overview',page,limit:25,search:q,sort}); } catch { /* Registry access remains independent of the delegated hosting service. */ }
  return <>
    <div className="page-heading"><div><h1>{t('Operations')}</h1><p className="muted">{t('Applications and recent hosting operations across customers.')}</p></div></div>
    <nav className="workspace-actions" aria-label={t('Operations areas')}><Link className="button secondary" href="/infrastructure">{t('Infrastructure')}</Link><Link className="button secondary" href="/activity">{t('Activity')}</Link></nav>
    {!data ? <p className="workspace-notice" role="alert">{t('Hosting status is currently unavailable.')}</p> : <>
      <HostingRefresh active={data.operations.some(operation=>['queued','running'].includes(operation.status))}/>
      <p className="muted">{t('Read from the operations journal at')} {date(data.readAt,{dateStyle:'short',timeStyle:'short'})}. {t('This is the last reported state, not a live availability check.')}</p>
      <section className="panel"><h2>{t('Applications')}</h2>
        <ListControls path="/operations" q={q} sort={sort} placeholder={t('Search customers, projects or applications')} sortOptions={[{value:'name',label:'Name A–Z'},{value:'-name',label:'Name Z–A'}]}/>
        <HostingTable rows={data.docs} rowKey={app=>app.id} empty={t('No matching applications.')} columns={[
          {label:t('Application'),render:app=><Link href={`/hosting/apps/${app.id}`}>{app.name}</Link>},
          {label:t('Customer'),render:app=><Link href={`/customers/${app.customerID}`}>{app.customerName}</Link>},
          {label:t('Project'),render:app=><Link href={`/hosting/projects/${app.projectID}`}>{app.projectName}</Link>},
          {label:t('Observed state'),render:app=>t(applicationState(app.status))},
          {label:t('Applied revision'),render:app=>`${app.observedRevision} / ${app.revision}`},
        ]}/><Pagination path="/operations" query={{q,sort}} {...data}/>
      </section>
      <section className="panel"><h2>{t('Recent operations')}</h2><p className="muted">{t('Up to 30 operations, with pending work and required reviews first. Search above filters applications.')}</p>
        <HostingTable rows={data.operations} rowKey={operation=>operation.id} empty={t('No operations recorded.')} columns={[
          {label:t('Operation'),render:operation=><Link href={`/hosting/operations/${operation.id}`}><code>{operation.action}</code></Link>},
          {label:t('Customer'),render:operation=>operation.customerName??'—'},
          {label:t('Project'),render:operation=>operation.projectName??'—'},
          {label:t('Status'),render:operation=>t(uiLabel(operation.status))},
          {label:t('Created'),render:operation=>date(operation.createdAt,{dateStyle:'short',timeStyle:'short'})},
        ]}/>
      </section>
    </>}
  </>;
}
