import {getRequestI18n} from '@webdock/i18n/next';
export default async function MailLoading(){const i18n=await getRequestI18n();return <p role="status" aria-live="polite">{i18n.t('Loading Mail…')}</p>;}
