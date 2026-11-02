'use client';
import Link from 'next/link';
import {useActionState,useState} from 'react';
import {useI18n} from '@webdock/i18n/react';
import type {NativeSendingView} from '@/lib/mail-sending';
import {sendingAction} from './actions';
export function SendingPanel({data}:{data:NativeSendingView}){
 const i18n=useI18n();const [result,submit,pending]=useActionState(sendingAction,{});
 const [copied,setCopied]=useState(false);
 const state=data.transport?.state;
 const ready=state==='ready',canCreate=ready && data.domains.length>0;
 const labels={pending:i18n.t('Applying change'),ready:i18n.t('Ready'),needs_review:i18n.t('Operator review required'),disabled:i18n.t('Disabled')};
 return <div className="access-page">
  {result.error && <p className="notice error" role="alert">{i18n.error(result.error)}</p>}
  {result.message && !result.token && <p className="notice" role="status">{i18n.error(result.message,'Changes saved.')}</p>}
  {result.token && <section className="notice" role="status"><p>{i18n.t('Save this Webdock API key now. It is shown only once.')}</p><code style={{overflowWrap:'anywhere'}}>{result.token}</code><p><button className="button secondary" type="button" onClick={()=>{void navigator.clipboard.writeText(result.token!).then(()=>setCopied(true),()=>setCopied(false));}}>{copied?i18n.t('Copied'):i18n.t('Copy API key')}</button></p></section>}
  <section className="auth-panel"><h2>{i18n.t('Sending connection')}</h2>
   <p role="status">{state?labels[state]:i18n.t('Not configured')}</p>
   {!ready && <p className="muted">{state==='pending'?i18n.t('The sending connection is being prepared. Refresh this page to check progress.'):state==='needs_review'?i18n.t('Your operator must check the sending connection before you can create keys.'):i18n.t('Verify a sender domain, then configure the sending connection to create an API key.')}</p>}
   <form action={submit} className="access-form mailbox-form" aria-busy={pending}>
    <input type="hidden" name="customer" value={data.customerID}/><input type="hidden" name="action" value="configure"/>
    <fieldset disabled={pending || data.transport?.canConfigure===false}>
     <label>{i18n.t('Sending connection')}<select name="mode" defaultValue={data.transport?.mode??'managed'}><option value="managed">{i18n.t('Email provided by Webdock')}</option>{(data.byokAllowed || data.transport?.mode==='byok') && <option value="byok" disabled={!data.byokAllowed}>{i18n.t('Own sending connection')}</option>}</select></label>
     <button className="button">{pending?i18n.t('Saving…'):i18n.t('Configure sending')}</button>
    </fieldset>
   </form>
   {data.transport?.mode==='byok' && !data.byokAllowed && <p className="notice">{i18n.t('Access to your own sending connection has been revoked. Ask your operator to review it.')}</p>}
   {data.apiOrigin && <p style={{overflowWrap:'anywhere'}}>{i18n.t('Webdock sending endpoint')}: <code>{data.apiOrigin}/v1/emails</code></p>}
  </section>
  <section className="auth-panel"><h2>{i18n.t('Webdock API keys')}</h2>
   <p className="muted">{i18n.t('These keys can send mail and check their own submissions. They cannot read mailboxes.')}</p>
   {!data.domains.length && <p className="notice"><Link href={`/tenants/${data.customerID}/mail`}>{i18n.t('Verify a sender domain first.')}</Link></p>}
   <form action={submit} className="access-form mailbox-form" aria-busy={pending} onSubmit={()=>setCopied(false)}>
    <input type="hidden" name="customer" value={data.customerID}/><input type="hidden" name="action" value="create"/>
    <fieldset disabled={pending || !canCreate}>
     <label>{i18n.t('Label')}<input name="label" required maxLength={100}/></label>
     <label>{i18n.t('Sender domain')}<select name="domainID"><option value="">{i18n.t('All verified sender domains')}</option>{data.domains.map(domain=><option key={domain.id} value={domain.id}>{domain.domain}</option>)}</select></label>
     <button className="button">{pending?i18n.t('Saving…'):i18n.t('Create Webdock key')}</button>
    </fieldset>
   </form>
   {!data.keys.length && <p className="muted">{i18n.t('No Webdock API keys yet.')}</p>}
   {data.keys.map(key=><div key={key.id} className="panel"><strong>{key.label}</strong> <code>…{key.suffix}</code>{key.revokedAt?<p>{i18n.t('Revoked')}</p>:<form action={submit} onSubmit={event=>{if(!window.confirm(i18n.t('Revoke this key? Applications using it will no longer be able to send mail.')))event.preventDefault();}}><input type="hidden" name="customer" value={data.customerID}/><input type="hidden" name="action" value="revoke"/><input type="hidden" name="keyID" value={key.id}/><button className="button secondary" disabled={pending}>{i18n.t('Revoke key')}</button></form>}</div>)}
  </section>
 </div>;
}
