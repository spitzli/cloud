'use client';

import {useModuleView} from '@webdock/module-sdk/react';
import {moduleForPath} from '@webdock/module-sdk/selection';
import { ConsoleIcon, type ConsoleIconName } from './console-icon';
import { NavigationPending } from "./navigation-pending";
import Link from 'next/link';
import { usePathname, useSearchParams } from 'next/navigation';
import { useI18n } from '@webdock/i18n/react';
import { areaSelected } from '@/lib/workspace-navigation';

export type AreaLink = { href: string; label: string; exact?: boolean; activePaths?: string[]; nested?: boolean; icon?: ConsoleIconName };
export function AreaNavigation({ links, label }: { links: AreaLink[]; label?: string }) {
  const view = useModuleView();
  links = links.filter(link => { const module = moduleForPath(link.href.split('?')[0]); return !module || ['enabled','draining'].includes(view.modules[module]); });
  const i18n = useI18n();
  const path = usePathname(), search = useSearchParams().toString();
  const active = links.find(link => areaSelected(link.href, path, search, link.exact, link.activePaths));
  const items = links.map(link => <Link key={link.href} href={link.href} className={link.nested ? "area-link-nested" : undefined} aria-current={areaSelected(link.href, path, search, link.exact, link.activePaths) ? 'page' : undefined}>{link.icon&&<ConsoleIcon name={link.icon}/>}<span>{i18n.t(link.label)}</span><NavigationPending /></Link>);
  return <div className="area-navigation">
    <nav className="area-navigation-desktop" aria-label={label || i18n.t('Areas')}>{items}</nav>
    <details className="area-navigation-mobile" onClick={event => { if ((event.target as HTMLElement).closest('a')) event.currentTarget.open = false; }} onKeyDown={event => { if (event.key === 'Escape') { event.currentTarget.open = false; event.currentTarget.querySelector('summary')?.focus(); } }}>
      <summary>{i18n.t('Area')}: {active ? i18n.t(active.label) : i18n.t('Choose an area')}</summary>
      <nav aria-label={label || i18n.t('Areas')}>{items}</nav>
    </details>
  </div>;
}
