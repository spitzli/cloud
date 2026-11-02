import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { z } from "zod";
import { WarningCircleIcon } from "@phosphor-icons/react/dist/csr/WarningCircle";
import { useApi } from "./context";
import * as ui from "./ui";

const noticeSchema = z.object({
  id: z.string(),
  kind: z.enum(["crashing", "image", "refused", "certificate"]),
  plugin: z.string(),
  unit: z.string(),
  project: z.object({ id: z.string(), name: z.string() }).nullable(),
  resource: z.object({ id: z.string(), name: z.string() }).nullable(),
  openedAt: z.string(),
  resolvedAt: z.string().nullable(),
});
const what = {
  crashing: "keeps stopping right after it starts. Its logs say why.",
  image: "cannot start because its image could not be pulled. Check the image name and registry access.",
  refused: "was refused by the cluster. Check its settings or contact your provider.",
  certificate: "has no certificate yet. Check that its domains point at the platform.",
} as const;

/** What in this workspace needs attention right now, from Core's notices. Nothing when all is well. */
export function Attention({ customer }: { customer: string }) {
  const api = useApi();
  const notices = useQuery({
    queryKey: ["notifications", customer],
    queryFn: ({ signal }) => api.get(`/api/console/customers/${customer}/notifications`, z.array(noticeSchema), signal),
    refetchInterval: 60_000,
  });
  const open = notices.data?.filter((n) => !n.resolvedAt) ?? [];
  if (!open.length) return null;
  return (
    <section aria-label="Needs attention" className={ui.cx(ui.card, "mb-6 border-warning")}>
      {open.map((n) => (
        <div key={n.id} className={ui.cx(ui.cardRow, "first:border-t-0")}>
          <div className="flex min-w-0 items-start gap-3">
            <WarningCircleIcon size={20} className="mt-0.5 shrink-0 text-warning" />
            <p className="text-sm">
              {n.resource && n.project ? (
                <Link
                  to="/workspaces/$customer/projects/$id/resources/$plugin/$resource"
                  params={{ customer, id: n.project.id, plugin: n.plugin, resource: n.resource.id }}
                  className="font-medium"
                >
                  {n.resource.name}
                </Link>
              ) : (
                <span className="font-medium">{n.unit}</span>
              )}{" "}
              in {n.project?.name ?? "this workspace"} {what[n.kind]}
            </p>
          </div>
          <span className="text-xs text-muted">since {new Date(n.openedAt).toLocaleString()}</span>
        </div>
      ))}
    </section>
  );
}
