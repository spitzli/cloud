import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { z } from "zod";
import { ArrowLeftIcon } from "@phosphor-icons/react/dist/csr/ArrowLeft";
import { useApi, useSession } from "../context";
import { Empty, Failure, Loading } from "../common";
import { revisionSchema } from "./views";
import * as ui from "../ui";
import { OperatorResources } from "./OperatorResources";

const modes = [
  { value: "everyone", label: "Everyone" },
  { value: "selected", label: "Selected customers" },
  { value: "assigned", label: "Operator assigns" },
] as const;
type Mode = (typeof modes)[number]["value"];
const policySchema = z.object({
  revision: z.string(),
  features: z.array(z.object({
    id: z.string(),
    label: z.string(),
    description: z.string().optional(),
    offering: z.string().optional(),
    supported: z.boolean().default(true),
    mode: z.enum(["everyone", "selected", "assigned"]),
    customers: z.array(z.object({ id: z.string(), name: z.string() })),
  })),
});

/** Operator page of one plugin: global self-service settings. Which customers are selected is decided on each customer page. */
export function PluginSettings({ plugin }: { plugin: string }) {
  const api = useApi();
  const session = useSession();
  const client = useQueryClient();
  const allowed = session.user.role === "operator" && !session.user.preview;
  const path = `/api/console/plugins/${encodeURIComponent(plugin)}/self-service`;
  const policy = useQuery({ queryKey: ["self-service", plugin], queryFn: ({ signal }) => api.get(path, policySchema, signal), enabled: allowed });
  const change = useMutation({
    mutationFn: ({ url, body }: { url: string; body: Record<string, unknown> }) => api.write(url, "PUT", body, revisionSchema),
    onSettled: () => client.invalidateQueries({ queryKey: ["self-service", plugin] }),
  });
  if (!allowed) return <Empty title="Operator access required">Self-service settings are managed by an operator.</Empty>;
  if (policy.isPending) return <Loading>Loading self-service settings…</Loading>;
  if (policy.error) return <Failure error={policy.error} retry={() => void policy.refetch()} />;
  const { revision, features } = policy.data;
  const setMode = (feature: string, mode: Mode) => {
    change.mutate({ url: `${path}/features/${encodeURIComponent(feature)}`, body: { mode, expectedRevision: revision } });
  };
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <Link className={ui.backLink} to="/extensions">
            <ArrowLeftIcon />
            Installed extensions
          </Link>
          <h1 className={ui.pageTitle}>{plugin}</h1>
          <p className={ui.pageLead}>What customers use, and what they can do themselves.</p>
        </div>
      </div>
      <OperatorResources plugin={plugin} />
      <h2 className={ui.cx(ui.sectionTitle, "mb-1")}>Self-service</h2>
      <p className="mb-4 text-[13px] text-muted">Choose what customers can do themselves. With “Selected customers”, you allow each customer on their customer page.</p>
      {change.error ? <p role="alert" className="mb-4 text-sm text-danger">{change.error.message}</p> : null}
      {features.length === 0 ? (
        <Empty title="No self-service features">This extension is managed by operators only.</Empty>
      ) : (
        <ul className={ui.cx(ui.card, "divide-y divide-outline")} aria-label="Self-service">
          {features.map((feature) => (
            <li key={feature.id} className={ui.cx(ui.cardBody, "gap-3")}>
              <div className="flex flex-wrap items-start justify-between gap-4">
                <div>
                  <h2 className="text-sm font-semibold">{feature.label}</h2>
                  {feature.description ? <p className="text-[13px] text-muted">{feature.description}</p> : null}
                  {!feature.supported ? <p className="text-[13px] text-muted">Not configured for this installation.</p> : null}
                </div>
                <label className="grid gap-1 text-[13px] text-muted">
                  Who can do this themselves
                  <select
                    className={ui.cx(ui.input, "w-56")}
                    value={feature.mode}
                    disabled={change.isPending || !feature.supported}
                    onChange={(event) => { setMode(feature.id, z.enum(["everyone", "selected", "assigned"]).parse(event.target.value)); }}
                  >
                    {modes.map((mode) => <option key={mode.value} value={mode.value}>{mode.label}</option>)}
                  </select>
                </label>
              </div>
              {feature.mode === "selected" ? (
                <p className="text-[13px] text-muted">
                  {feature.customers.length === 0 ? "No customers yet." : `Allowed for ${feature.customers.map((c) => c.name).join(", ")}.`}{" "}
                  Choose customers on their customer page.
                </p>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
