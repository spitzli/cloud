import { ConsoleSidebar } from '@/components/console-sidebar';
export default async function Sidebar({params}:{params:Promise<{segments:string[]}>}) { return <ConsoleSidebar segments={(await params).segments}/>; }
