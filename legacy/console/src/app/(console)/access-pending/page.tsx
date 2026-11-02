import {getRequestI18n} from '@webdock/i18n/next';
import {AccountControls} from '@/components/account-controls';
import {clerkConsoleEnabled} from '@/lib/console-identity';
import {notFound} from 'next/navigation';
import {auth} from '@clerk/nextjs/server';
export default async function Pending(){
 if(!clerkConsoleEnabled())notFound();
 await auth.protect();
 const {t}=await getRequestI18n();
 return <main id="main" className="auth-panel"><h1>{t('Workspaces')}</h1><p>{t('You do not currently have tenant access. Accept your emailed invitation or contact your Webdock operator.')}</p><AccountControls /></main>;
}
