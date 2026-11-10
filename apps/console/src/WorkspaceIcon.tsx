import { msgid } from "@spitzli/i18n";
import { useI18n } from "@spitzli/i18n/react";
import { useRef } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { useApi } from "./context";
import { WorkspaceAvatar } from "./WorkspaceAvatar";
import * as ui from "./ui";

async function prepareIcon(file: File) {
  if (!["image/png", "image/jpeg", "image/webp"].includes(file.type) || !file.size || file.size > 2 * 1024 * 1024) {
    throw new Error(msgid("Choose a PNG, JPEG or WebP image up to 2 MiB."));
  }
  let image: ImageBitmap;
  try { image = await createImageBitmap(file); }
  catch { throw new Error(msgid("This image could not be read. Choose another image.")); }
  try {
    if (image.width > 4096 || image.height > 4096) throw new Error(msgid("Choose an image no larger than 4096 × 4096 pixels."));
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 64;
    const context = canvas.getContext("2d");
    if (!context) throw new Error(msgid("Image processing is unavailable in this browser."));
    const scale = Math.min(64 / image.width, 64 / image.height);
    const width = image.width * scale, height = image.height * scale;
    context.drawImage(image, (64 - width) / 2, (64 - height) / 2, width, height);
    return canvas.toDataURL("image/png");
  } finally { image.close(); }
}

/** Uses the existing tenant-management permission and audit trail. */
export function WorkspaceIcon({ customer, name, icon }: { customer: string; name: string; icon?: string | null | undefined }) {
  const api = useApi();
  const { t } = useI18n();
  const client = useQueryClient();
  const input = useRef<HTMLInputElement>(null);
  const change = useMutation({
    mutationFn: async (file: File | null) => api.write(`/api/console/tenant/${encodeURIComponent(customer)}`, "POST", { action: "icon", icon: file ? await prepareIcon(file) : null }, z.object({ message: z.string() })),
    onSuccess: async () => {
      await Promise.all(["tenant", "tenants", "account-tenants"].map(key => client.invalidateQueries({ queryKey: key === "tenant" ? [key, customer] : [key] })));
    },
    onSettled: () => { if (input.current) input.current.value = ""; },
  });
  return <section className={ui.card} aria-label={t("Workspace icon")}>
    <div className={ui.cardBody}>
      <h2 className={ui.sectionTitle}>{t("Workspace icon")}</h2>
      <p className="text-sm text-muted">{t("Use your team’s logo across Spitzli Cloud. Without an icon, we show the first letter of the workspace name.")}</p>
      <div className="flex flex-wrap items-center gap-4">
        <WorkspaceAvatar name={name} icon={icon} size="size-16 text-xl" />
        <div className="grid gap-2">
          <label className={ui.field}>{t("Upload workspace icon")}
            <input ref={input} type="file" accept="image/png,image/jpeg,image/webp" disabled={change.isPending} onChange={event => {
              const file = event.currentTarget.files?.[0];
              if (file) change.mutate(file);
            }} className="max-w-full text-sm file:mr-3 file:rounded-md file:border file:border-outline file:bg-soft file:px-3 file:py-2 file:text-foreground" />
          </label>
          <p className={ui.hint}>{t("PNG, JPEG or WebP, up to 2 MiB and 4096 × 4096 pixels. Saved automatically as a 64 × 64 PNG.")}</p>
        </div>
        {icon ? <button className={ui.button()} disabled={change.isPending} onClick={() => { change.mutate(null); }}>{t("Remove workspace icon")}</button> : null}
      </div>
      {change.isPending ? <p role="status" className={ui.hint}>{t("Saving workspace icon…")}</p> : change.isSuccess ? <p role="status" className={ui.hint}>{t("Workspace icon saved.")}</p> : null}
      {change.error ? <p role="alert" className="text-sm text-danger">{t(change.error.message)}</p> : null}
    </div>
  </section>;
}
