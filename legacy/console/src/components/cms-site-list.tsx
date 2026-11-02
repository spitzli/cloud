"use client";
import {useOramaRows} from "@webdock/search/react";
import { useI18n } from '@webdock/i18n/react';

import { useState } from "react";
import Link from "next/link";
import { cmsRoleLabel } from "@/lib/cms-modules";

type Website = { id: string; name: string; domain: string; role: string; href: string; central: boolean; disabled?: boolean };
export function CmsSiteList({ sites }: { sites: Website[] }) {
  const i18n = useI18n();

  const [query, setQuery] = useState("");
  const visible=useOramaRows(sites,query,site=>`${site.name} ${site.domain}`);
  return <section className="cms-directory-list" aria-label={i18n.t("Websites")}>
    <div className="cms-directory-toolbar"><label><span className="cms-visually-hidden">{i18n.t("Search websites")}</span><input type="search" value={query} onChange={e => setQuery(e.target.value)} placeholder={i18n.t("Search websites…")} autoComplete="off" /></label><span role="status">{visible.length} {visible.length === 1 ? i18n.t("Website") : i18n.t("Websites")}</span></div>
    <div className="cms-directory-columns" aria-hidden="true"><span>{i18n.t("Website")}</span><span>{i18n.t("Access")}</span><span>{i18n.t("Management")}</span></div>
    {visible.map(site => {
      const content = <><div className="cms-directory-site"><span className="cms-site-initial" aria-hidden="true">{site.name.slice(0,1).toLocaleUpperCase(i18n.locale)}</span><div><strong>{site.name}</strong><span>{site.domain}</span></div></div><span className="cms-role">{i18n.t(cmsRoleLabel(site.role))}</span><span className="cms-directory-open">{site.central ? i18n.t("Open CMS") : i18n.t("Website CMS")}<span aria-hidden="true">{site.central ? "›" : "↗"}</span></span></>;
      return site.disabled ? <div className="cms-directory-row" key={site.id} aria-disabled="true"><div className="cms-directory-site"><div><strong>{site.name}</strong><span>{i18n.t("Preview is unavailable for this website CMS")}</span></div></div><span className="cms-role">{i18n.t("View only")}</span></div> : site.central ? <Link className="cms-directory-row" key={site.id} href={site.href}>{content}</Link> : <a className="cms-directory-row" key={site.id} href={site.href}>{content}</a>;
    })}
    {!visible.length && <div className="cms-empty-state"><h2>{i18n.t("No websites found")}</h2><p>{i18n.t("Try another name or domain.")}</p><button className="button secondary" onClick={() => setQuery("")}>{i18n.t("Reset search")}</button></div>}
  </section>;
}
