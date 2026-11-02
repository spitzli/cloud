import Mail from '../mail-workspace';
import {MailResourcePage} from '@/components/mail/resource-page';
import {loadMailPage,type MailPageProps} from '@/components/mail/product-page';
export default async function Page(props:MailPageProps){
 const context=await loadMailPage(props,'/domains');
 return context.connection?<MailResourcePage props={props} kind="domains"/>:<Mail {...props} view="domains"/>;
}
