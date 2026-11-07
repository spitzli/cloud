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

/** What in this workspace needs attention right now, from Core's notices. Show explicit loading, failure and empty states. */
export function Attention({ customer }: { customer: string }) {
  const api = useApi();
  const notices = useQuery({
    queryKey: ["notifications", customer],
    queryFn: ({ signal }) => api.get(`/api/console/customers/${customer}/notifications`, z.array(noticeSchema), signal),
    staleTime: 60_000,
    refetchOnWindowFocus: false,
    retry: false,
  });
  const open = notices.isSuccess ? notices.data.filter((n) => !n.resolvedAt) : [];
  return (
    <section aria-label="Needs attention" className={ui.cx(ui.card, open.length > 0 && "border-warning")}>
      <div className={ui.cardBody}>
        <h2 className={ui.sectionTitle}>Needs attention</h2>
        {notices.isPending ? <p role="status" className="text-sm text-muted">Checking alerts…</p> : notices.isError ? <div className="grid gap-2"><p className="text-sm text-muted">Alerts are unavailable.</p><button className={ui.button()} onClick={() => { void notices.refetch(); }}>Retry alerts</button></div> : !open.length ? <p className="text-sm text-muted">No open alerts.</p> : null}
      </div>
      {open.map((n) => (
        <div key={n.id} className={ui.cx(ui.cardRow, "first:border-t-0")}>
          <div className="flex min-w-0 items-start gap-3">
            <WarningCircleIcon size={20} className="mt-0.5 shrink-0 text-warning" />
            <p className="min-w-0 text-sm [overflow-wrap:anywhere]">
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
