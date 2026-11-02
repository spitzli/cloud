import { ConsoleToolbar } from '@/components/console-toolbar';
export default async function Toolbar({ params }: { params: Promise<{segments?:string[]}> }) {
  return <ConsoleToolbar segments={(await params).segments} />;
}
