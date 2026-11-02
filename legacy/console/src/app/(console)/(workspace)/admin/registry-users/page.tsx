import {browserAuthConfiguration} from "@/lib/browser-auth-config";
import Link from 'next/link';
import {getRequestI18n} from '@webdock/i18n/next';
import {requireRegistryOperator} from '@/lib/registry-page';
export default async function RegistryUsers({searchParams}:{searchParams:Promise<{page?:string}>}){
 const {payload}=await requireRegistryOperator(),i18n=await getRequestI18n(),params=await searchParams;
 const clerk=browserAuthConfiguration.provider === "clerk";
 const page=/^[1-9]\d{0,3}$/.test(params.page||'')?Number(params.page):1;
 const result=await payload.find({collection:'users',page,limit:25,depth:0,sort:'email'});
 return <div className="access-page"><header className="account-heading"><div><h1>{i18n.t('Studio accounts')}</h1>{browserAuthConfiguration.provider!=="clerk"&&<p className="muted">{i18n.t('Customer invitations and website access are managed in People & access.')}</p>}</div>{browserAuthConfiguration.provider!=="clerk"&&<Link className="button secondary" href="/people">{i18n.t('People & access')}</Link>}</header>
  {clerk&&<section className="panel"><p>{i18n.t('Roles and memberships are managed in Clerk. Local records only link identities to Webdock resources.')}</p><a href="https://dashboard.clerk.com" target="_blank" rel="noopener noreferrer">{i18n.t('Open Clerk administration')}</a></section>}
  {result.docs.map(user=><section className="panel" key={user.id}><h2>{user.name}</h2><p className="muted">{user.id}</p><p>{user.email}{!clerk&&<> · {user.role}</>}</p></section>)}
  <nav aria-label={i18n.t('Pagination')}>{result.hasPrevPage&&<Link href={'?page='+(page-1)}>{i18n.t('Previous')}</Link>} {result.hasNextPage&&<Link href={'?page='+(page+1)}>{i18n.t('Next')}</Link>}</nav>
 </div>;
}
