import type {Metadata} from 'next';
import {getRequestI18n} from '@spitzli/i18n/next';
import {I18nProvider} from '@spitzli/i18n/react';
import {LanguagePicker} from '@spitzli/i18n/picker';
export async function generateMetadata():Promise<Metadata>{const {t}=await getRequestI18n();return {title:t('Website administration — Webdock'),robots:{index:false,follow:false}};}
export default async function Layout({children}:{children:React.ReactNode}){const {locale,preference}=await getRequestI18n();return <html lang={locale}><body style={{fontFamily:'system-ui, sans-serif',margin:0,colorScheme:'light dark'}}><I18nProvider locale={locale} preference={preference}><div style={{display:'flex',justifyContent:'flex-end',padding:16}}><LanguagePicker/></div>{children}</I18nProvider></body></html>;}
