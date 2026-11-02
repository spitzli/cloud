import Mail from '../mail-workspace';
import {getRequestI18n} from '@webdock/i18n/next';
import {MailConnectionSettings} from '@/components/mail/connection-settings';
import type {MailPageProps} from '@/components/mail/product-page';
export default async function Page(props:MailPageProps){const i18n=await getRequestI18n();return <div className="access-page"><header className="account-heading"><h1>{i18n.t('Mail settings')}</h1></header><MailConnectionSettings props={props}/><details className="auth-panel"><summary>{i18n.t('Sending account')}</summary><Mail {...props} view="settings" embedded/></details></div>;}
