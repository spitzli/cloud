
import type { TenantPreview } from "@/lib/studio-client";
import { TenantPreviewBanner } from "./tenant-preview";

export async function StudioShell({
  children,
  preview,
}: {
  children: React.ReactNode;
  preview?: TenantPreview;
}) {

  return (
    <div className="workspace">
      <div className="main-wrap">
        {preview && <TenantPreviewBanner preview={preview}/>}
        <main id="main">{children}</main>

      </div>
    </div>
  );
}
