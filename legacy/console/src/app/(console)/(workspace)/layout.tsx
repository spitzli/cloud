import { requireRegistryOperator } from "../../../lib/registry-page";
import { StudioShell } from "../../../components/studio-shell";
export const dynamic = "force-dynamic";
export default async function Layout({
  children,
}: {
  children: React.ReactNode;
}) {
  await requireRegistryOperator();
  return (
    <StudioShell>
      {children}
    </StudioShell>
  );
}
