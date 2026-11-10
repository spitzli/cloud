import type {Metadata} from 'next';
import localFont from 'next/font/local';
import {getRequestI18n} from '@spitzli/i18n/next';
import {I18nProvider} from '@spitzli/i18n/react';
import {LanguagePicker} from '@spitzli/i18n/picker';
import '@spitzli/cms-ui/styles.css';
const space=localFont({src:'../../../public/fonts/space-grotesk.ttf',variable:'--font-space',display:'swap'});
export async function generateMetadata():Promise<Metadata>{const {t}=await getRequestI18n();return {title:t('CMS — Spitzli Cloud'),robots:{index:false,follow:false},referrer:'same-origin'};}
export default async function Layout({children}:{children:React.ReactNode}){const {locale,preference}=await getRequestI18n();return <html lang={locale} className={space.variable}><body><I18nProvider locale={locale} preference={preference}><div className="cms-ui-language"><LanguagePicker/></div>{children}</I18nProvider></body></html>;}
