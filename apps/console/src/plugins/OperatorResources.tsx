import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { z } from "zod";
import { useApi } from "../context";
import { Failure, Loading } from "../common";
import { useNativeUI } from "./native";
import * as ui from "../ui";

const resourcesSchema = z.array(z.object({
  id: z.string(),
  offering: z.string(),
  offeringLabel: z.string().nullable(),
  name: z.string(),
  createdVia: z.enum(["operator", "self-service"]),
  customer: z.object({ id: z.string(), name: z.string() }),
  project: z.object({ id: z.string(), name: z.string() }),
})).max(1000);

/** Every resource of one plugin across customers, plus the plugin's own operator UI if it ships one. */
export function OperatorResources({ plugin }: { plugin: string }) {
  const api = useApi();
  const native = useNativeUI(plugin);
  const query = useQuery({
    queryKey: ["plugin-resources", plugin],
    queryFn: ({ signal }) => api.get(`/api/console/plugins/${encodeURIComponent(plugin)}/resources`, resourcesSchema, signal),
  });
  if (query.isPending || native.isLoading) return <Loading>Loading resources…</Loading>;
  if (query.error) return <Failure error={query.error} retry={() => void query.refetch()} />;
  const Native = native.data?.Operator;
  return (
    <div className="mb-8 grid gap-6">
      {Native ? <Native plugin={plugin} resources={query.data} /> : null}
      <section className={ui.card} aria-label="Resources">
        <h2 className={ui.cx(ui.sectionTitle, "px-6 py-4 max-sm:px-[18px]")}>Resources</h2>
        {query.data.length === 0 ? (
          <p className="px-6 pb-5 text-sm text-muted max-sm:px-[18px]">No customer uses this extension yet.</p>
        ) : (
          <ul>
            {query.data.map((resource) => (
              <li key={resource.id} className={ui.cx(ui.cardRow, ui.clickableRow)}>
                <Link
                  className={ui.cx(ui.rowLink, "font-medium")}
                  to="/workspaces/$customer/projects/$id/resources/$plugin/$resource"
                  params={{ customer: resource.customer.id, id: resource.project.id, plugin, resource: resource.id }}
                >
                  {resource.name}
                </Link>
                <span className="text-[13px] text-muted">
                  {resource.offeringLabel ?? resource.offering} in {resource.customer.name} / {resource.project.name}
                  {resource.createdVia === "operator" ? ", assigned" : ""}
                </span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
