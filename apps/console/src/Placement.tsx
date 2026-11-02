import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { useApi } from "./context";
import { Empty, Failure, Loading } from "./common";
import * as ui from "./ui";

const clusterSchema = z.object({ id: z.string(), name: z.string(), region: z.string(), country: z.string().nullable(), dedicated: z.boolean() });
const placementSchema = z.object({ cluster: z.string().nullable(), options: z.array(clusterSchema), movable: z.boolean() });
const where = (c: z.infer<typeof clusterSchema>) => [c.region, c.country].filter(Boolean).join(", ");

/** Where a project's services run. Shown only when the workspace has a choice. */
export function Placement({ projectID, role }: { projectID: string; role: string | null | undefined }) {
  const api = useApi();
  const client = useQueryClient();
  const placement = useQuery({
    queryKey: ["placement", projectID],
    queryFn: ({ signal }) => api.get(`/api/console/projects/${projectID}/placement`, placementSchema, signal),
  });
  const place = useMutation({
    mutationFn: (clusterId: string | null) => api.write(`/api/console/projects/${projectID}/placement`, "PUT", { clusterId }, placementSchema),
    onSuccess: (data) => { client.setQueryData(["placement", projectID], data); },
  });
  const data = placement.data;
  if (!data || (!data.options.length && !data.cluster)) return null;
  const writable = ["owner", "admin", "operator"].includes(role ?? "");
  const current = data.options.find((c) => c.id === data.cluster);
  return (
    <section className={ui.cx(ui.card, "mt-8")} aria-label="Runs on">
      <div className={ui.cardBody}>
        <div>
          <h2 className={ui.sectionTitle}>Runs on</h2>
          <p className={ui.hint}>
            {data.movable ? "New services in this project start on this cluster." : "Services already run here, so the project stays on its cluster."}
          </p>
        </div>
        {writable && data.movable ? (
          <label className={ui.field}>
            Cluster
            <select className={ui.cx(ui.input, "max-w-96")} value={data.cluster ?? ""} disabled={place.isPending} onChange={(e) => { place.mutate(e.target.value || null); }}>
              <option value="">Platform default</option>
              {data.options.map((c) => (
                <option key={c.id} value={c.id}>{`${c.name} (${where(c)})${c.dedicated ? ", yours alone" : ""}`}</option>
              ))}
            </select>
          </label>
        ) : (
          <p className="text-sm">{current ? `${current.name} (${where(current)})` : data.cluster ? "A cluster chosen earlier" : "Platform default"}</p>
        )}
        {place.isError ? <p role="alert" className="text-sm text-danger">{place.error.message}</p> : null}
      </div>
    </section>
  );
}

const operatorClusterSchema = z.object({
  id: z.string(),
  name: z.string(),
  region: z.string(),
  country: z.string().nullable(),
  verified: z.boolean(),
  offered: z.boolean(),
  dedicatedTo: z.object({ id: z.string(), name: z.string() }).nullable(),
  projects: z.number(),
});
/** Operators: which clusters customers may choose for their projects. */
export function Clusters() {
  const api = useApi();
  const client = useQueryClient();
  const clusters = useQuery({ queryKey: ["clusters"], queryFn: ({ signal }) => api.get("/api/console/clusters", z.array(operatorClusterSchema), signal) });
  const offer = useMutation({
    mutationFn: ({ id, offered }: { id: string; offered: boolean }) => api.write(`/api/console/clusters/${id}`, "PUT", { offered }, z.object({})),
    onSuccess: () => void client.invalidateQueries({ queryKey: ["clusters"] }),
  });
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>Clusters</h1>
          <p className={ui.pageLead}>Offered clusters appear as a choice on every project; a cluster dedicated to a customer is that customer&apos;s choice only. A project moves only while nothing runs in it.</p>
        </div>
      </div>
      {clusters.isPending ? <Loading>Loading clusters…</Loading> : null}
      {clusters.isError ? <Failure error={clusters.error} retry={() => void clusters.refetch()} /> : null}
      {clusters.data && !clusters.data.length ? <Empty title="No clusters yet">Enroll a cluster's agent first.</Empty> : null}
      {clusters.data?.length ? (
        <div className={ui.card}>
          {clusters.data.map((c) => (
            <div key={c.id} className={ui.cardRow}>
              <div className="min-w-0">
                <span className="font-medium">{c.name}</span>
                <p className="text-xs text-muted">
                  {[c.region, c.country].filter(Boolean).join(", ")}
                  {c.verified ? "" : ", not verified"}
                  {c.dedicatedTo ? `, dedicated to ${c.dedicatedTo.name}` : ""}
                  {`, ${c.projects === 1 ? "1 project chose it" : `${String(c.projects)} projects chose it`}`}
                </p>
              </div>
              {c.dedicatedTo ? null : (
                <label className="flex items-center gap-2 text-sm">
                  <input type="checkbox" checked={c.offered} disabled={!c.verified || offer.isPending} onChange={(e) => { offer.mutate({ id: c.id, offered: e.target.checked }); }} />
                  Offered
                </label>
              )}
            </div>
          ))}
        </div>
      ) : null}
      {offer.isError ? <p role="alert" className="mt-3 text-sm text-danger">{offer.error.message}</p> : null}
    </section>
  );
}
