import {MailGatewayOverview} from '@/components/mail/gateway-overview';
import {getTenantMailService} from '@/lib/mail-service';
import type {MailPageProps} from '@/components/mail/product-page';
import {headers} from 'next/headers';
import Link from 'next/link';
import {getRequestI18n} from '@webdock/i18n/next';
import {getNativeSending} from '@/lib/mail-sending';
import {SendingPanel} from './panel';
export default async function SendingPage(props:MailPageProps){
 const {customerID}=await props.params,i18n=await getRequestI18n();
 const [data,native]=await Promise.all([getNativeSending(await headers(),customerID),getTenantMailService(await headers(),customerID)]);
 return <div className="access-page"><header className="account-heading"><h1>{i18n.t('Webdock Mail sending')}</h1><Link href={`/tenants/${customerID}/mail`}>{i18n.t('Back to Mail')}</Link></header><MailGatewayOverview props={props} nativeIncoming={!!native&&native.service.revision!=='0'}/>{data?<SendingPanel data={data}/>:<p>{i18n.t('Native Mail sending is not available yet.')}</p>}</div>;
}
