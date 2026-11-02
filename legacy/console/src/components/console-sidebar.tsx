import {browserAuthConfiguration} from "@/lib/browser-auth-config";
import { getConsoleContext } from '@/lib/console-context';
import { Navigation } from './navigation';
import { WorkspaceSidebar } from './workspace-sidebar';
export async function ConsoleSidebar({segments=[]}:{segments?:string[]}) {
  const context=await getConsoleContext(segments.join('/'));
  if(!context)return null;
  return <aside className="sidebar"><div className="desktop-navigation"><Navigation accountProvider={browserAuthConfiguration.provider} operator={context.session.operator} preview={!!context.session.preview}/><WorkspaceSidebar segments={segments}/></div></aside>;
}
