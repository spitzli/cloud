"use client";
import { AreaNavigation } from "./area-navigation";
export function CmsWorkspaceNav({ links }: { links: { label: string; href: string }[] }) {
  return <AreaNavigation links={links.map(link => ({ ...link, exact: true }))} />;
}
