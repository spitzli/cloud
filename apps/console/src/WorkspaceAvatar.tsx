import { useState } from "react";

/** Workspace branding never loads a third-party image or exposes member browsing. */
export function WorkspaceAvatar({ name, icon, size = "size-9 text-sm" }: { name: string; icon?: string | null | undefined; size?: string }) {
  const [failed, setFailed] = useState<string>();
  const source = icon?.startsWith("data:image/png;base64,") && icon.length <= 32768 ? icon : undefined;
  return (
    <span aria-hidden className={`grid shrink-0 place-items-center overflow-hidden rounded-full bg-accent font-semibold text-on-accent ${size}`}>
      {source && source !== failed ? <img src={source} alt="" className="size-full object-contain" onError={() => { setFailed(source); }} /> : Array.from(name.trim())[0]?.toUpperCase() ?? "?"}
    </span>
  );
}
