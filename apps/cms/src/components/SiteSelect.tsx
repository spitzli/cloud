'use client';
import { useEffect, useRef } from 'react';
import {useI18n} from '@spitzli/i18n/react';
export function SiteSelect({sites}:{sites:{id:number;name:string;tenant:number;locale:string}[]}) {
 const {t}=useI18n();
 const ref=useRef<HTMLSelectElement>(null);
 useEffect(()=>{const match=document.cookie.split('; ').find(c=>c.startsWith('cms-site='));if(ref.current)ref.current.value=match?.split('=')[1]||'';},[]);
 return <label style={{display:'grid',gap:8,padding:16}}>{t("Site")}<select ref={ref} defaultValue="" onChange={event=>{
  const selected=sites.find(site=>String(site.id)===event.target.value);
  document.cookie=`cms-site=${selected?.id||''}; Path=/; SameSite=Lax; Secure`;
  document.cookie=`payload-tenant=${selected?.tenant||''}; Path=/; SameSite=Lax; Secure`;
  if(selected)document.cookie=`payload-locale=${selected.locale}; Path=/; SameSite=Lax; Secure`;
  window.location.assign(selected?`/admin?locale=${selected.locale}`:'/admin');
 }}><option value="">{t("All accessible sites")}</option>{sites.map(site=><option key={site.id} value={site.id}>{site.name}</option>)}</select></label>;
}
