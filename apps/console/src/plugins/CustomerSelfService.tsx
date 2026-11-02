import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { useApi } from "../context";
import { Loading } from "../common";
import { revisionSchema } from "./views";
import * as ui from "../ui";

const customerSchema = z.array(z.object({
  plugin: z.string(),
  pluginRevision: z.string(),
  feature: z.string(),
  label: z.string(),
  mode: z.enum(["everyone", "selected", "assigned"]),
  allowed: z.boolean(),
}));
/** On a customer's page: every self-service feature, with a checkbox where the mode is "Selected customers". */
export function CustomerSelfService({ customer }: { customer: string }) {
  const api = useApi();
  const client = useQueryClient();
  const query = useQuery({
    queryKey: ["customer-self-service", customer],
    queryFn: ({ signal }) => api.get(`/api/console/customers/${encodeURIComponent(customer)}/self-service`, customerSchema, signal),
  });
  const change = useMutation({
    mutationFn: (row: z.infer<typeof customerSchema>[number]) =>
      api.write(`/api/console/plugins/${encodeURIComponent(row.plugin)}/self-service/features/${encodeURIComponent(row.feature)}/customers/${encodeURIComponent(customer)}`, "PUT", { allowed: !row.allowed, expectedRevision: row.pluginRevision }, revisionSchema),
    onSettled: () => client.invalidateQueries({ queryKey: ["customer-self-service", customer] }),
  });
  if (query.isPending) return <Loading>Loading self-service…</Loading>;
  if (query.error) return null;
  if (!query.data.length) return null;
  const label = { everyone: "Everyone", assigned: "Operator assigns", selected: "Selected customers" } as const;
  return (
    <section className={ui.cx(ui.card, ui.cardBody, "mt-6 gap-3")} aria-label="Self-service">
      <h2 className={ui.sectionTitle}>Self-service</h2>
      <p className="text-[13px] text-muted">What this customer can do themselves.</p>
      {change.error ? <p role="alert" className="text-sm text-danger">{change.error.message}</p> : null}
      <ul className="grid gap-2">
        {query.data.map((row) => (
          <li key={`${row.plugin}:${row.feature}`} className="flex flex-wrap items-center justify-between gap-3 text-sm">
            {row.mode === "selected" ? (
              <label className="flex items-center gap-2">
                <input type="checkbox" className="size-4 accent-accent" checked={row.allowed} disabled={change.isPending} onChange={() => { change.mutate(row); }} />
                {row.label}
              </label>
            ) : (
              <span>{row.label}</span>
            )}
            <span className="text-[13px] text-muted">
              {row.plugin} · {label[row.mode]}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}
