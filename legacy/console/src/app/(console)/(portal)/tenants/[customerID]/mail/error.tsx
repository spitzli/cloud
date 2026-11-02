'use client';
import {useI18n} from '@webdock/i18n/react';
export default function MailError({reset}:{reset:()=>void}){const i18n=useI18n();return <section className="auth-panel"><h1>{i18n.t('Mail is temporarily unavailable.')}</h1><p role="alert">{i18n.t('Your messages are retained. Refresh the page to load the current state.')}</p><button onClick={reset}>{i18n.t('Try again')}</button></section>;}
