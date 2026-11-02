'use client';
import { useEffect, useRef, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from 'react';
import Link from 'next/link';
import { useI18n } from '@webdock/i18n/react';
import { useOramaRows } from '@webdock/search/react';
import type { ConsoleSearchResponse, ConsoleResult } from '@/lib/console-search';
import { ConsoleIcon } from './console-icon';

type Scope = { customerID?: string; customerName?: string; projectID?: string; resourceName?: string };
const resultIcons = {navigation:'overview',workspace:'overview',website:'website',project:'apps',application:'apps',database:'database'} as const;
async function querySearch<T>(params: URLSearchParams, signal: AbortSignal): Promise<T> {
  const response = await fetch('/console/search?' + params, { signal, cache: 'no-store' });
  if (!response.ok) throw new Error('Search unavailable');
  return response.json();
}
function navigateResults(event: KeyboardEvent<HTMLDialogElement>) {
  if (!['ArrowDown','ArrowUp'].includes(event.key) || (event.target instanceof HTMLSelectElement)) return;
  const links = [...event.currentTarget.querySelectorAll<HTMLAnchorElement>('[data-console-result]')];
  if (!links.length) return;
  event.preventDefault();
  const position = links.indexOf(document.activeElement as HTMLAnchorElement);
  links[position < 0 ? (event.key === 'ArrowDown' ? 0 : links.length - 1) : (position + (event.key === 'ArrowDown' ? 1 : -1) + links.length) % links.length].focus();
}
function closeOnNavigation(event: MouseEvent, dialog: HTMLDialogElement | null) {
  if (!event.defaultPrevented && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey) dialog?.close();
}

export function ConsoleControls({ scope, workspaces, navigation, account, operator, preview }: {
  scope: Scope;
  workspaces: {id:string;name:string}[];
  navigation: ReactNode;
  account: ReactNode;
  operator: boolean;
  preview: boolean;
}) {
  const { t } = useI18n();
  const searchDialog = useRef<HTMLDialogElement>(null), pickerDialog = useRef<HTMLDialogElement>(null), menuDialog = useRef<HTMLDialogElement>(null);
  const searchInput = useRef<HTMLInputElement>(null), pickerInput = useRef<HTMLInputElement>(null);
  const [searchOpen,setSearchOpen] = useState(false), [pickerOpen,setPickerOpen] = useState(false);
  const [query,setQuery] = useState(''), [pickerQuery,setPickerQuery] = useState('');
  const [searchScope,setSearchScope] = useState(scope.projectID?'project':scope.customerID?'workspace':'all');
  const [pickerTab,setPickerTab] = useState<'workspaces'|'projects'>(scope.projectID?'projects':'workspaces');
  const [response,setResponse] = useState<ConsoleSearchResponse>({results:[]});
  const [pending,setPending] = useState(false);
  const [projects,setProjects] = useState<{id:string;name:string}[]>([]), [projectsLoading,setProjectsLoading] = useState(false), [projectsError,setProjectsError] = useState(false), [projectsMore,setProjectsMore] = useState(false);
  const visibleWorkspaces=useOramaRows(workspaces,pickerQuery,item=>`${item.name} ${item.id}`);
  const openSearch=()=>{setPending(true);setQuery('');setResponse({results:[]});searchDialog.current?.showModal();setSearchOpen(true);searchInput.current?.focus();};
  const openPicker=()=>{setProjectsError(false);setProjectsLoading(pickerTab==='projects');setPickerQuery('');pickerDialog.current?.showModal();setPickerOpen(true);pickerInput.current?.focus();};
  useEffect(()=>{
    const shortcut=(event:globalThis.KeyboardEvent)=>{
      if(event.key.toLowerCase()==='k'&&(event.metaKey||event.ctrlKey)&&!event.isComposing){
        event.preventDefault();pickerDialog.current?.close();menuDialog.current?.close();
        if (!searchDialog.current?.open) { searchDialog.current?.showModal();setSearchOpen(true);setPending(true); }
        searchInput.current?.focus();
      }
    };
    document.addEventListener('keydown',shortcut);return()=>document.removeEventListener('keydown',shortcut);
  },[]);
  useEffect(()=>{
    if(!searchOpen)return;
    let current=true;
    const controller = new AbortController();
    const timer=setTimeout(()=>{
      setPending(true);
      const request={query,...(searchScope!=='all'?{customerID:scope.customerID}:{}),...(searchScope==='project'?{projectID:scope.projectID}:{})};
      querySearch<ConsoleSearchResponse>(new URLSearchParams(request),controller.signal).then(value=>{if(current)setResponse(value);}).catch(()=>{if(current)setResponse({results:[],error:t('Search is temporarily unavailable. Please try again.')});}).finally(()=>{if(current)setPending(false);});
    },180);
    return()=>{current=false;clearTimeout(timer);controller.abort();};
  },[query,searchOpen,searchScope,scope.customerID,scope.projectID,t]);
  useEffect(()=>{
    if(!pickerOpen||pickerTab!=='projects'||!scope.customerID)return;
    let current=true;
    const controller = new AbortController();
    const timer=setTimeout(()=>{
      setProjectsLoading(true);setProjectsError(false);
      querySearch<{projects:{id:string;name:string}[];more?:boolean;error?:boolean}>(new URLSearchParams({kind:"projects",customerID:scope.customerID!,query:pickerQuery}),controller.signal).then(value=>{if(current){setProjects(value.projects);setProjectsMore(!!value.more);setProjectsError(!!value.error);}}).catch(()=>{if(current)setProjectsError(true);}).finally(()=>{if(current)setProjectsLoading(false);});
    },180);
    return()=>{current=false;clearTimeout(timer);controller.abort();};
  },[pickerOpen,pickerTab,pickerQuery,scope.customerID]);
  const kinds: [ConsoleResult['kind'],string][]=[['navigation',t('Go to')],['workspace',t('Workspaces')],['project',t('Projects')],['website',t('Websites')],['application',t('Applications')],['database',t('Databases')]];
  return <>
    <header className="console-toolbar">
      <button type="button" className="console-icon-button console-menu-toggle" aria-label={t('Open navigation')} onClick={()=>menuDialog.current?.showModal()}><ConsoleIcon name="menu" /></button>
      <Link className="brand console-brand" href={operator?'/':'/tenants'}>webdock<span>.</span><small>{t('Console')}</small></Link>
      <button type="button" className="console-scope-button" onClick={openPicker} aria-haspopup="dialog" title={t('Switch workspace or project')}>
        <ConsoleIcon name="apps" /><span><strong>{scope.customerName||t(operator?'Operator area':'Select workspace')}</strong>{scope.resourceName&&<small>{scope.resourceName}</small>}</span><ConsoleIcon name="chevron" />
      </button>
      <button type="button" className="console-search-button" aria-label={t('Search resources and services')} onClick={openSearch} aria-haspopup="dialog" aria-keyshortcuts="Control+k Meta+k"><ConsoleIcon name="search" /><span>{t('Search resources and services')}</span><kbd>Ctrl K</kbd></button>
      <div className="console-account">{account}</div>
    </header>
    <dialog ref={searchDialog} className="console-dialog console-search-dialog" aria-labelledby="console-search-title" onClose={()=>{setSearchOpen(false);setResponse({results:[]});setPending(false);}} onKeyDown={navigateResults}>
      <div className="console-dialog-heading"><h2 id="console-search-title">{t('Search resources and services')}</h2><button className="console-icon-button" type="button" aria-label={t('Close search')} onClick={()=>searchDialog.current?.close()}><ConsoleIcon name="close" /></button></div>
      <div className="console-search-field"><ConsoleIcon name="search" /><input ref={searchInput} type="search" aria-label={t('Search resources and services')} placeholder={t('Name or domain')} value={query} maxLength={160} onChange={event=>{setPending(true);setQuery(event.target.value);setResponse({results:[]});}} autoComplete="off" /></div>
      <div className="console-search-scope"><label>{t('Search in')}<select value={searchScope} onChange={event=>{setPending(true);setSearchScope(event.target.value);setResponse({results:[]});}}>
        {scope.projectID&&<option value="project">{t('Current project')}</option>}{scope.customerID&&<option value="workspace">{scope.customerName||t('Current workspace')}</option>}<option value="all">{preview?t('Available resources'):t('All workspaces')}</option>
      </select></label><span>{searchScope==='project'?t('Applications, databases and navigation'):searchScope==='workspace'?t('Projects, websites and navigation'):t('Workspaces, websites and navigation')}</span></div>
      <div className="console-results" aria-busy={pending}>
        {pending&&<p role="status" className="console-search-status">{t('Searching…')}</p>}
        {response.error&&<p role="alert" className="console-search-status">{response.error}</p>}
        {!pending&&!response.error&&query.trim().length>=2&&!response.results.length&&<p className="console-search-status">{t('No results. Try a different name or change the search scope.')}</p>}
        {kinds.map(([kind,label])=>{const results=response.results.filter(result=>result.kind===kind);return results.length>0&&<section key={kind} aria-label={label}><h3>{label}</h3><ul>{results.map(result=><li key={result.id}>
          {result.external?<a href={result.href} data-console-result target="_blank" rel="noreferrer" onClick={event=>closeOnNavigation(event,searchDialog.current)}><ConsoleIcon name={resultIcons[result.kind]} /><span><strong>{result.label}</strong><small>{result.description}</small></span><span aria-hidden="true">↗</span></a>
            :<Link href={result.href} prefetch={false} data-console-result onClick={event=>closeOnNavigation(event,searchDialog.current)}><ConsoleIcon name={resultIcons[result.kind]} /><span><strong>{result.label}</strong>{result.description&&<small>{result.description}</small>}</span><span aria-hidden="true">↵</span></Link>}
        </li>)}</ul></section>;})}
        {response.partial&&<p role="status" className="console-search-status">{t('Some resources could not be loaded. Try the search again.')}</p>}
        {response.more&&<p className="console-search-status">{t('More results are available. Narrow your search.')}</p>}
      </div>
      <footer className="console-dialog-footer"><span>{t('Type at least two characters to find resources.')}</span><span><kbd>↑</kbd><kbd>↓</kbd> {t('Navigate')} <kbd>Esc</kbd> {t('Close')}</span></footer>
    </dialog>
    <dialog ref={pickerDialog} className="console-dialog console-picker-dialog" aria-labelledby="console-picker-title" onClose={()=>{setPickerOpen(false);setProjectsLoading(false);}} onKeyDown={navigateResults}>
      <div className="console-dialog-heading"><h2 id="console-picker-title">{t('Switch workspace or project')}</h2><button type="button" className="console-icon-button" aria-label={t('Close workspace selector')} onClick={()=>pickerDialog.current?.close()}><ConsoleIcon name="close" /></button></div>
      <div className="console-picker-tabs"><button type="button" aria-pressed={pickerTab==='workspaces'} onClick={()=>{setPickerTab('workspaces');setPickerQuery('');setProjectsLoading(false);}}>{t('Workspaces')}</button>{scope.customerID&&<button type="button" aria-pressed={pickerTab==='projects'} onClick={()=>{if(pickerTab!=='projects'||pickerQuery){setPickerTab('projects');setPickerQuery('');setProjects([]);setProjectsLoading(true);}}}>{t('Projects')}</button>}</div>
      <div className="console-search-field"><ConsoleIcon name="search" /><input ref={pickerInput} type="search" maxLength={160} value={pickerQuery} onChange={event=>{setPickerQuery(event.target.value);if(pickerTab==='projects'){setProjects([]);setProjectsLoading(true);}}} placeholder={t('Filter by name')} aria-label={t('Filter workspaces and projects')} /></div>
      <div className="console-results" aria-busy={projectsLoading&&pickerTab==='projects'}>
        {pickerTab==='workspaces'?<ul>{visibleWorkspaces.map(item=><li key={item.id}><Link href={`/tenants/${item.id}`} data-console-result prefetch={false} aria-current={item.id===scope.customerID?'true':undefined} onClick={event=>closeOnNavigation(event,pickerDialog.current)}><ConsoleIcon name="overview" /><span><strong>{item.name}</strong><small>{item.id}</small></span>{item.id===scope.customerID&&<span>{t('Selected')}</span>}</Link></li>)}</ul>
          :<><p className="console-search-status">{scope.customerName}</p>{projectsLoading&&<p role="status" className="console-search-status">{t('Loading projects…')}</p>}{projectsError&&<p role="alert" className="console-search-status">{t('Projects could not be loaded. Please try again.')}</p>}<ul>{projects.map(item=><li key={item.id}><Link href={operator ? `/projects/${item.id}` : `/hosting/projects/${item.id}`} data-console-result prefetch={false} aria-current={item.id===scope.projectID?'true':undefined} onClick={event=>closeOnNavigation(event,pickerDialog.current)}><ConsoleIcon name="apps" /><span><strong>{item.name}</strong><small>{item.id}</small></span>{item.id===scope.projectID&&<span>{t('Selected')}</span>}</Link></li>)}</ul>{!projectsLoading&&!projectsError&&!projects.length&&<p className="console-search-status">{t('No matching projects in this workspace.')}</p>}{projectsMore&&<p className="console-search-status">{t('More results are available. Narrow your search.')}</p>}</>}
        {pickerTab==='workspaces'&&!visibleWorkspaces.length&&<p className="console-search-status">{t('No matching workspaces.')}</p>}
      </div>
      <footer className="console-dialog-footer"><Link href={pickerTab==='projects'?`/tenants/${scope.customerID}/hosting`:'/tenants?choose=1'} onClick={event=>closeOnNavigation(event,pickerDialog.current)}>{t('Open full overview')}</Link></footer>
    </dialog>
    <dialog ref={menuDialog} className="console-drawer" aria-labelledby="console-menu-title" onClick={event=>{if((event.target as HTMLElement).closest('a'))closeOnNavigation(event,menuDialog.current);}}>
      <div className="console-dialog-heading"><h2 id="console-menu-title">{t('Navigation')}</h2><button type="button" className="console-icon-button" aria-label={t('Close navigation')} onClick={()=>menuDialog.current?.close()}><ConsoleIcon name="close" /></button></div>
      {navigation}
    </dialog>
  </>;
}
