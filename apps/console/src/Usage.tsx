import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Link } from "@tanstack/react-router";
import { useI18n } from "@webdock/i18n/react";
import { allowanceSchema, formatResource, hostingDimensions, hostingLabels } from "@webdock/hosting-contracts";
import { z } from "zod";
import { useApi } from "./context";
import { Empty, Failure, Loading } from "./common";
import { useHostingCommand } from "./hosting-client";
import * as ui from "./ui";

const amounts = z.record(z.string(), z.number().nullable());
const meterSchema = z.object({ id: z.string(), label: z.string(), unit: z.string(), plugin: z.string().optional() });
type Meter = z.infer<typeof meterSchema>;
const usageSchema = z.object({
  plan: z.object({ id: z.string(), name: z.string(), description: z.string(), assigned: z.boolean() }).nullable(),
  overrides: amounts,
  limits: amounts,
  allocated: amounts,
  meters: z.array(meterSchema),
  units: z.array(
    z.object({
      plugin: z.string(),
      unit: z.string(),
      project: z.object({ id: z.string(), name: z.string() }).nullable(),
      resource: z.object({ id: z.string(), name: z.string(), offering: z.string() }).nullable(),
      allocation: amounts,
      usage: z.object({ cpu: z.number(), memory: z.number() }).nullish(),
      observedAt: z.string().nullable(),
    }),
  ),
  history: z.array(z.object({ hour: z.string(), cpu: z.number(), memory: z.number() })),
});
type Usage = z.infer<typeof usageSchema>;
const planSchema = z.object({
  id: z.string(),
  name: z.string(),
  description: z.string(),
  limits: amounts,
  isDefault: z.boolean(),
  customers: z.number(),
});
type Plan = z.infer<typeof planSchema>;
const plansSchema = z.object({ plans: z.array(planSchema), meters: z.array(meterSchema) });
const hostingDemandSchema = allowanceSchema.pipe(z.record(z.enum(hostingDimensions), z.number().int().min(0).max(Number.MAX_SAFE_INTEGER)));
const hostingUsageSchema = z.object({
  effective: allowanceSchema.required(),
  allocated: hostingDemandSchema,
  reserved: hostingDemandSchema,
});

/** 1536 MiB → "1.5 GiB", 250 millicores → "0.25 CPU", counts as they are. */
export function amount(meter: Pick<Meter, "unit">, value: number) {
  if (meter.unit === "MiB") return value >= 1024 ? `${String(+(value / 1024).toFixed(1))} GiB` : `${String(value)} MiB`;
  if (meter.unit === "millicores") return `${String(+(value / 1000).toFixed(value < 100 ? 3 : 2))} CPU`;
  return String(value);
}
function useUsage(customer: string, live = true) {
  const api = useApi();
  return useQuery({
    queryKey: ["usage", customer],
    queryFn: ({ signal }) => api.get(`/api/console/customers/${customer}/usage`, usageSchema, signal),
    refetchInterval: live ? 60_000 : false,
    ...(live ? {} : { refetchOnWindowFocus: false, staleTime: 60_000, retry: false }),
  });
}

function Bar({ used, limit }: { used: number; limit: number }) {
  const share = limit === 0 ? (used > 0 ? 1 : 0) : Math.min(used / limit, 1);
  const tone = used > limit ? "bg-danger" : share >= 0.8 ? "bg-warning" : "bg-accent";
  return (
    <div className="h-1.5 w-full overflow-hidden rounded-full bg-soft" aria-hidden>
      <div className={`h-full rounded-full ${tone}`} style={{ width: `${String(share * 100)}%` }} />
    </div>
  );
}
/** Each meter the plan limits, or that is in use, with what is taken against the limit. */
function Meters({ usage }: { usage: Usage }) {
  const { t } = useI18n();
  const rows = usage.meters.filter((m) => usage.limits[m.id] != null || (usage.allocated[m.id] ?? 0) > 0);
  if (!rows.length) return <p className="text-sm text-muted">{t("No resource reservations have been reported yet.")}</p>;
  return (
    <ul className="grid gap-4 sm:grid-cols-2">
      {rows.map((meter) => {
        const used = usage.allocated[meter.id] ?? 0;
        const limit = usage.limits[meter.id];
        return (
          <li key={meter.id} className="grid gap-1.5">
            <div className="flex items-baseline justify-between gap-3 text-sm">
              <span className="font-medium">{meter.plugin ? `${meter.label} (${meter.plugin})` : meter.label}</span>
              <span className={used > (limit ?? Infinity) ? "text-danger" : "text-muted"}>
                {limit == null ? <>{amount(meter, used)} {t("reserved · no plan limit")}</> : <>{amount(meter, used)} of {amount(meter, limit)}</>}
              </span>
            </div>
            {limit == null ? <div className="h-1.5 rounded-full bg-soft" aria-hidden /> : <Bar used={used} limit={limit} />}
          </li>
        );
      })}
    </ul>
  );
}
/** Hourly totals as a line; hours without samples are gaps, not zeros. */
function Trend({ points, label, format, end }: { points: { hour: string; value: number }[]; label: string; format: (v: number) => string; end: number }) {
  const start = end - 7 * 24 * 3600_000;
  const peak = points.length ? Math.max(...points.map((p) => p.value)) : 0;
  const max = Math.max(1, peak);
  const x = (hour: string) => ((new Date(hour).getTime() - start) / (end - start)) * 100;
  const y = (value: number) => 38 - (value / max) * 34;
  const segments: string[] = [];
  let current = "";
  let last = 0;
  for (const point of points) {
    const at = new Date(point.hour).getTime();
    if (current && at - last > 3600_000) {
      segments.push(current);
      current = "";
    }
    current += `${current ? "L" : "M"}${x(point.hour).toFixed(2)},${y(point.value).toFixed(2)}`;
    last = at;
  }
  if (current) segments.push(current);
  // A lone sample still shows, as a dot.
  const drawn = segments.map((d) => (d.includes("L") ? d : `${d}h0.4`));
  const latest = points.at(-1)?.value;
  return (
    <figure className="grid gap-1.5">
      <figcaption className="flex items-baseline justify-between text-sm">
        <span className="font-medium">{label}</span>
        <span className="text-muted">
          {latest == null ? "No samples yet" : `Now ${format(latest)}, peak ${format(peak)}`}
        </span>
      </figcaption>
      <svg viewBox="0 0 100 40" preserveAspectRatio="none" className="h-20 w-full rounded-md border border-outline bg-subtle" role="img" aria-label={`${label} over the last 7 days`}>
        {drawn.map((d) => (
          <path key={d} d={d} fill="none" stroke="var(--accent)" strokeWidth="2" strokeLinecap="round" vectorEffect="non-scaling-stroke" />
        ))}
      </svg>
      <div className="flex justify-between text-xs text-muted">
        <span>7 days ago</span>
        <span>Now</span>
      </div>
    </figure>
  );
}
/** Every running unit: what it reserves and what it uses right now. */
function Units({ usage }: { usage: Usage }) {
  if (!usage.units.length) return null;
  const mib = { unit: "MiB" };
  const cpu = { unit: "millicores" };
  return (
    <div className={ui.tableFrame}>
      <table className={ui.table}>
        <thead>
          <tr>
            <th className={ui.th}>Service</th>
            <th className={ui.th}>Project</th>
            <th className={ui.th}>Memory reserved</th>
            <th className={ui.th}>Memory used</th>
            <th className={ui.th}>CPU reserved</th>
            <th className={ui.th}>CPU used</th>
            <th className={ui.th}>Storage</th>
          </tr>
        </thead>
        <tbody>
          {usage.units.map((u) => (
            <tr key={`${u.plugin}/${u.unit}`} className={ui.tr}>
              <td className={ui.cx(ui.td, "min-w-48 [overflow-wrap:normal]")}>
                <span className="font-medium">{u.resource?.name ?? u.unit}</span>
                <span className="block text-xs text-muted">{u.plugin}</span>
              </td>
              <td className={ui.td}>{u.project?.name ?? "Workspace"}</td>
              <td className={ui.td}>{amount(mib, u.allocation.memory ?? 0)}</td>
              <td className={ui.td}>{u.usage ? amount(mib, u.usage.memory) : "—"}</td>
              <td className={ui.td}>{amount(cpu, u.allocation.cpu ?? 0)}</td>
              <td className={ui.td}>{u.usage ? amount(cpu, u.usage.cpu) : "—"}</td>
              <td className={ui.td}>{amount(mib, u.allocation.storage ?? 0)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
function useHostingReservations(customer: string, live = true) {
  const call = useHostingCommand();
  return useQuery({
    queryKey: ["hosting", "usage", customer],
    queryFn: ({ signal }) => call({ action: "usage.get", customerID: customer }, hostingUsageSchema, signal),
    refetchInterval: live ? 60_000 : false,
    retry: false,
    ...(live ? {} : { refetchOnWindowFocus: false, staleTime: 60_000 }),
  });
}
function NativeHostingUsage({ customer }: { customer: string }) {
  const { t, locale } = useI18n();
  const usage = useHostingReservations(customer);
  return <section className={ui.card} aria-label={t("Application hosting")}>
    <div className={ui.cardBody}>
      <h2 className={ui.sectionTitle}>{t("Application hosting")}</h2>
      <p className={ui.hint}>{t("Customer-wide hosting allowances. Project or provider limits may be stricter.")}</p>
      {usage.isPending ? <Loading>{t("Loading hosting reservations…")}</Loading> : usage.isError ? <Failure title={t("Unable to load hosting reservations")} error={usage.error} retry={() => { void usage.refetch(); }} /> : <>
        <div className={ui.tableFrame}><table className={ui.table}>
          <thead><tr><th className={ui.th} scope="col">{t("Resource")}</th><th className={ui.th} scope="col">{t("Allocated")}</th><th className={ui.th} scope="col">{t("Pending reservation")}</th><th className={ui.th} scope="col">{t("Customer allowance")}</th></tr></thead>
          <tbody>{hostingDimensions.map(dimension => <tr key={dimension} className={ui.tr}>
            <th scope="row" className={ui.td}>{t(hostingLabels[dimension])}</th>
            <td className={ui.td}>{formatResource(dimension, usage.data.allocated[dimension], locale)}</td>
            <td className={ui.td}>{formatResource(dimension, usage.data.reserved[dimension], locale)}</td>
            <td className={ui.td}>{usage.data.effective[dimension] === null ? t("No customer limit") : formatResource(dimension, usage.data.effective[dimension], locale)}</td>
          </tr>)}</tbody>
        </table></div>
        <p className={ui.hint}>{t("Allocated and pending reservations describe configured resources, not live measurements. Replicas per app shows the maximum in each state.")}</p>
      </>}
    </div>
  </section>;
}

/** Application hosting and extension usage retain their distinct allowance and measurement scopes. */
export function PlanUsage({ customer }: { customer: string }) {
  const api = useApi();
  const { t } = useI18n();
  return <div className="grid gap-8">
    {api.config.enabledFeatures.includes("hosting") ? <NativeHostingUsage customer={customer} /> : null}
    <section aria-label={t("Extension plan and workload usage")} className="grid gap-4">
      <div><h2 className={ui.sectionTitle}>{t("Project and extension usage")}</h2><p className={ui.hint}>{t("These plan meters cover projects and extensions. Workload measurements below belong to extensions; application hosting is shown separately.")}</p></div>
      <ExtensionUsage customer={customer} />
    </section>
  </div>;
}

function ExtensionUsage({ customer }: { customer: string }) {
  const { t } = useI18n();
  const usage = useUsage(customer);
  if (usage.isPending) return <Loading>Loading usage…</Loading>;
  if (usage.isError) return <Failure error={usage.error} retry={() => void usage.refetch()} />;
  const data = usage.data;
  return (
    <div className="grid gap-8">
      <section className={ui.card}>
        <div className={ui.cardBody}>
          <div>
            <h2 className={ui.sectionTitle}>{data.plan?.name ?? "No plan"}</h2>
            <p className={ui.hint}>
              {data.plan ? data.plan.description || "Limits of this workspace." : t("No workspace plan is assigned. Service-specific limits may still apply.")}
            </p>
          </div>
          <Meters usage={data} />
        </div>
        <div className={ui.cardFooter}>
          Reserved is what services are set up to take; used is what they took at the last measurement.
        </div>
      </section>
      <section className="grid gap-4">
        <h2 className={ui.sectionTitle}>Last 7 days</h2>
        <div className="grid gap-6 sm:grid-cols-2">
          <Trend end={usage.dataUpdatedAt} label="Memory used" format={(v) => amount({ unit: "MiB" }, v)} points={data.history.map((h) => ({ hour: h.hour, value: h.memory }))} />
          <Trend end={usage.dataUpdatedAt} label="CPU used" format={(v) => amount({ unit: "millicores" }, v)} points={data.history.map((h) => ({ hour: h.hour, value: h.cpu }))} />
        </div>
      </section>
      <section className="grid gap-4">
        <h2 className={ui.sectionTitle}>Services</h2>
        {data.units.length ? <Units usage={data} /> : <p className="text-sm text-muted">{t("No service usage has been reported yet.")}</p>}
      </section>
    </div>
  );
}

/** Compact overview; reuse the authorized usage cache without an idle polling loop. */
export function WorkspaceUsageSummary({ customer }: { customer: string }) {
  const api = useApi();
  const native = api.config.enabledFeatures.includes("hosting");
  const usage = useUsage(customer, false);
  const data = usage.data;
  const meters = data?.meters.filter((m) => data.limits[m.id] != null || (data.allocated[m.id] ?? 0) > 0) ?? [];
  return (
    <section aria-label="Plan and usage" className={ui.card}>
      <div className={ui.cx(ui.cardBody, "gap-5")}>
        <div className="flex items-center justify-between gap-3">
          <h2 className={ui.sectionTitle}>Plan and usage</h2>
          <Link className="text-xs text-muted underline underline-offset-4" to="/workspaces/$id/usage" params={{ id: customer }}>View usage</Link>
        </div>
        {usage.isPending ? <p role="status" className="text-sm text-muted">Loading usage…</p> : usage.isError ? (
          <div className="grid gap-2"><p className="text-sm text-muted">Usage is unavailable.</p><button className={ui.button()} onClick={() => { void usage.refetch(); }}>Retry usage</button></div>
        ) : data ? <>
          <p className="text-lg font-semibold">{data.plan?.name ?? "No plan assigned"}</p>
          <p className={ui.hint}>Project and extension reservations. External providers bill separately.</p>
          {meters.length ? <ul className="grid gap-4" aria-label="Project and extension reservations">{meters.map((meter) => {
            const used = data.allocated[meter.id] ?? 0;
            const limit = data.limits[meter.id];
            return <li key={meter.id} className="grid gap-2">
              <div className="flex flex-wrap justify-between gap-2 text-[13px]"><span>{meter.label}</span><span className="text-muted">{amount(meter, used)}{limit == null ? " · no plan limit" : ` of ${amount(meter, limit)}`}</span></div>
              {limit == null ? null : <Bar used={used} limit={limit} />}
            </li>;
          })}</ul> : <p className="text-sm text-muted">No reservations reported.</p>}
        </> : null}
        {native ? <HostingReservationSummary customer={customer} /> : null}
      </div>
    </section>
  );
}
function HostingReservationSummary({ customer }: { customer: string }) {
  const { t, locale } = useI18n();
  const usage = useHostingReservations(customer, false);
  if (usage.isPending) return <p role="status" className="text-sm text-muted">Loading hosting reservations…</p>;
  if (usage.isError) return <div className="grid gap-2"><p className="text-sm text-muted">Hosting usage is unavailable.</p><button className={ui.button()} onClick={() => { void usage.refetch(); }}>Retry hosting usage</button></div>;
  return <div className="grid gap-4" aria-label="Hosting reservations">
    <p className={ui.hint}>Application resources, allocated and pending.</p>
    {(["apps", "cpuMillicores", "memoryBytes"] as const).map((dimension) => {
      const used = usage.data.allocated[dimension] + usage.data.reserved[dimension];
      const limit = usage.data.effective[dimension];
      return <div key={dimension} className="grid gap-2">
        <div className="flex flex-wrap justify-between gap-2 text-[13px]"><span>{t(hostingLabels[dimension])}</span><span className="text-muted">{formatResource(dimension, used, locale)}{limit == null ? " · no customer limit" : ` of ${formatResource(dimension, limit, locale)}`}</span></div>
        {limit == null ? null : <Bar used={used} limit={limit} />}
      </div>;
    })}
  </div>;
}

/** The usage page of one workspace. */
export function WorkspaceUsage({ id }: { id: string }) {
  const { t } = useI18n();
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>Plan and usage</h1>
          <p className={ui.pageLead}>{t("Hosting allowances and reservations, plus project and extension usage.")}</p>
        </div>
      </div>
      <PlanUsage customer={id} />
    </section>
  );
}

type Choice = "plan" | "custom" | "unlimited";
const choices: Choice[] = ["plan", "custom", "unlimited"];
/** Operators: which plan a customer is on, and limits that differ from it. */
export function CustomerPlan({ customer }: { customer: string }) {
  const api = useApi();
  const client = useQueryClient();
  const usage = useUsage(customer);
  const plans = useQuery({ queryKey: ["plans"], queryFn: ({ signal }) => api.get("/api/console/plans", plansSchema, signal) });
  const [draft, setDraft] = useState<{ plan: string; overrides: Record<string, { choice: Choice; value: string }> } | null>(null);
  const save = useMutation({
    mutationFn: (body: { planId: string | null; overrides: Record<string, number | null> }) =>
      api.write(`/api/console/customers/${customer}/plan`, "PUT", body, z.object({})),
    onSuccess: () => {
      setDraft(null);
      void client.invalidateQueries({ queryKey: ["usage", customer] });
      void client.invalidateQueries({ queryKey: ["plans"] });
    },
  });
  if (usage.isPending || plans.isPending) return <Loading>Loading plan…</Loading>;
  if (usage.isError) return <Failure error={usage.error} retry={() => void usage.refetch()} />;
  if (plans.isError) return <Failure error={plans.error} retry={() => void plans.refetch()} />;
  const override = (v: number | null): { choice: Choice; value: string } => (v == null ? { choice: "unlimited", value: "" } : { choice: "custom", value: String(v) });
  const current = {
    plan: usage.data.plan?.assigned ? usage.data.plan.id : "",
    overrides: Object.fromEntries(Object.entries(usage.data.overrides).map(([k, v]) => [k, override(v)])),
  };
  const form = draft ?? current;
  const fallback = plans.data.plans.find((p) => p.isDefault);
  const base = plans.data.plans.find((p) => p.id === form.plan) ?? fallback;
  const submit = (event: { preventDefault: () => void }) => {
    event.preventDefault();
    const overrides: Record<string, number | null> = {};
    for (const [meter, o] of Object.entries(form.overrides)) {
      if (o.choice === "unlimited") overrides[meter] = null;
      if (o.choice === "custom" && o.value.trim() !== "") overrides[meter] = Number(o.value);
    }
    save.mutate({ planId: form.plan || null, overrides });
  };
  const set = (next: typeof form) => { setDraft(next); };
  return (
    <form className={ui.cx(ui.card, "mt-8")} onSubmit={submit}>
      <div className={ui.cardBody}>
        <div>
          <h2 className={ui.sectionTitle}>Plan and limits</h2>
          <p className={ui.hint}>Core refuses what would take this customer past a limit. Billing is not part of Core.</p>
        </div>
        <label className={ui.field}>
          Plan
          <select className={ui.input} value={form.plan} onChange={(e) => { set({ ...form, plan: e.target.value }); }}>
            <option value="">{fallback ? `Default plan (${fallback.name})` : "No workspace plan"}</option>
            {plans.data.plans.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
        <div className={ui.tableFrame}>
          <table className={ui.table}>
            <thead>
              <tr>
                <th className={ui.th}>Limit</th>
                <th className={ui.th}>In use</th>
                <th className={ui.th}>For this customer</th>
              </tr>
            </thead>
            <tbody>
              {plans.data.meters.map((meter) => {
                const o = form.overrides[meter.id] ?? { choice: "plan", value: "" };
                const planned = base?.limits[meter.id];
                const change = (next: Partial<typeof o>) => { set({ ...form, overrides: { ...form.overrides, [meter.id]: { ...o, ...next } } }); };
                return (
                  <tr key={meter.id} className={ui.tr}>
                    <td className={ui.td}>
                      {meter.label}
                      {meter.plugin ? <span className="block text-xs text-muted">{meter.plugin}</span> : null}
                    </td>
                    <td className={ui.td}>{amount(meter, usage.data.allocated[meter.id] ?? 0)}</td>
                    <td className={ui.td}>
                      <div className="flex flex-wrap gap-2">
                        <select aria-label={`${meter.label} limit`} className={ui.cx(ui.input, "w-auto")} value={o.choice} onChange={(e) => { change({ choice: choices.find((c) => c === e.target.value) ?? "plan" }); }}>
                          <option value="plan">Plan: {planned == null ? "unlimited" : amount(meter, planned)}</option>
                          <option value="custom">Custom</option>
                          <option value="unlimited">Unlimited</option>
                        </select>
                        {o.choice === "custom" ? (
                          <input aria-label={`${meter.label} custom limit (${meter.unit})`} className={ui.cx(ui.input, "w-32")} inputMode="numeric" pattern="[0-9]{1,15}" required value={o.value} onChange={(e) => { change({ value: e.target.value }); }} placeholder={meter.unit} />
                        ) : null}
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </div>
      <div className={ui.cardFooter}>
        <span role={save.isError ? "alert" : undefined} className={save.isError ? "text-danger" : undefined}>
          {save.isError ? save.error.message : "Memory and storage in MiB, CPU in millicores."}
        </span>
        <div className="flex gap-2">
          {draft ? (
            <button type="button" className={ui.button("quiet")} onClick={() => { setDraft(null); }}>
              Discard
            </button>
          ) : null}
          <button className={ui.button("primary")} disabled={!draft || save.isPending}>
            Save plan
          </button>
        </div>
      </div>
    </form>
  );
}

interface PlanDraft { id?: string; name: string; description: string; isDefault: boolean; limits: Record<string, string> }
function PlanForm({ draft, meters, close }: { draft: PlanDraft; meters: Meter[]; close: () => void }) {
  const api = useApi();
  const client = useQueryClient();
  const [form, setForm] = useState(draft);
  const save = useMutation({
    mutationFn: () => {
      const limits = Object.fromEntries(Object.entries(form.limits).filter(([, v]) => v.trim() !== "").map(([k, v]) => [k, Number(v)]));
      const body = { name: form.name, description: form.description, isDefault: form.isDefault, limits };
      return form.id
        ? api.write(`/api/console/plans/${form.id}`, "PUT", body, z.object({ id: z.string() }))
        : api.write("/api/console/plans", "POST", body, z.object({ id: z.string() }));
    },
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["plans"] });
      void client.invalidateQueries({ queryKey: ["usage"] });
      close();
    },
  });
  const remove = useMutation({
    mutationFn: () => api.request(`/api/console/plans/${form.id ?? ""}`, z.object({}), { method: "DELETE" }),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["plans"] });
      close();
    },
  });
  const error = save.error ?? remove.error;
  return (
    <form
      className={ui.card}
      onSubmit={(e) => {
        e.preventDefault();
        save.mutate();
      }}
    >
      <div className={ui.cardBody}>
        <h2 className={ui.sectionTitle}>{form.id ? `Edit ${draft.name}` : "New plan"}</h2>
        <label className={ui.field}>
          Name
          <input className={ui.input} required maxLength={80} value={form.name} onChange={(e) => { setForm({ ...form, name: e.target.value }); }} />
        </label>
        <label className={ui.field}>
          Description
          <span className={ui.hint}>Customers see it on their usage page.</span>
          <textarea className={ui.input} maxLength={500} rows={2} value={form.description} onChange={(e) => { setForm({ ...form, description: e.target.value }); }} />
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={form.isDefault} onChange={(e) => { setForm({ ...form, isDefault: e.target.checked }); }} />
          Default plan for customers without one
        </label>
        <fieldset className="grid gap-3 sm:grid-cols-2">
          <legend className={ui.legend}>Limits (leave empty for unlimited)</legend>
          {meters.map((meter) => (
            <label key={meter.id} className={ui.field}>
              {meter.plugin ? `${meter.label} (${meter.plugin})` : meter.label}
              <input className={ui.input} inputMode="numeric" pattern="[0-9]{1,15}" placeholder={`Unlimited · ${meter.unit}`} value={form.limits[meter.id] ?? ""} onChange={(e) => { setForm({ ...form, limits: { ...form.limits, [meter.id]: e.target.value } }); }} />
            </label>
          ))}
        </fieldset>
      </div>
      <div className={ui.cardFooter}>
        <span role={error ? "alert" : undefined} className={error ? "text-danger" : undefined}>
          {error ? error.message : "Memory and storage in MiB, CPU in millicores."}
        </span>
        <div className="flex gap-2">
          {form.id ? (
            <button type="button" className={ui.button("quiet")} disabled={remove.isPending} onClick={() => { remove.mutate(); }}>
              Delete plan
            </button>
          ) : null}
          <button type="button" className={ui.button("quiet")} onClick={close}>
            Cancel
          </button>
          <button className={ui.button("primary")} disabled={save.isPending}>
            Save plan
          </button>
        </div>
      </div>
    </form>
  );
}
/** Operators: the plans customers can be on. */
export function Plans() {
  const api = useApi();
  const plans = useQuery({ queryKey: ["plans"], queryFn: ({ signal }) => api.get("/api/console/plans", plansSchema, signal) });
  const [editing, setEditing] = useState<PlanDraft | null>(null);
  const draft = (plan?: Plan): PlanDraft =>
    plan
      ? { id: plan.id, name: plan.name, description: plan.description, isDefault: plan.isDefault, limits: Object.fromEntries(Object.entries(plan.limits).map(([k, v]) => [k, String(v)])) }
      : { name: "", description: "", isDefault: false, limits: {} };
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>Plans</h1>
          <p className={ui.pageLead}>Define how many projects and services a workspace can use, and its CPU, memory and storage limits.</p>
        </div>
        {!editing ? (
          <button className={ui.button("primary")} onClick={() => { setEditing(draft()); }}>
            New plan
          </button>
        ) : null}
      </div>
      {plans.isPending ? <Loading>Loading plans…</Loading> : null}
      {plans.isError ? <Failure error={plans.error} retry={() => void plans.refetch()} /> : null}
      {plans.data && editing ? <PlanForm key={editing.id ?? "new"} draft={editing} meters={plans.data.meters} close={() => { setEditing(null); }} /> : null}
      {plans.data && !editing ? (
        plans.data.plans.length ? (
          <div className={ui.card}>
            {plans.data.plans.map((plan) => (
              <div key={plan.id} className={ui.cx(ui.cardRow, ui.clickableRow)}>
                <div className="min-w-0">
                  <button className={ui.cx("border-0 bg-transparent p-0 text-left font-medium", ui.rowLink)} onClick={() => { setEditing(draft(plan)); }}>
                    {plan.name}
                  </button>
                  <p className="text-xs text-muted">
                    {plans.data.meters
                      .filter((m) => plan.limits[m.id] != null)
                      .map((m) => `${m.label} ${amount(m, plan.limits[m.id] ?? 0)}`)
                      .join(", ") || "No workspace plan limits"}
                  </p>
                </div>
                <div className="flex items-center gap-3 text-xs text-muted">
                  {plan.isDefault ? <span className={ui.badge("accent")}>Default</span> : null}
                  {plan.customers === 1 ? "1 customer" : `${String(plan.customers)} customers`}
                </div>
              </div>
            ))}
          </div>
        ) : (
          <Empty title="No plans yet">No workspace plan limits are configured. Service-specific limits may still apply. Create a default plan for workspaces without an assigned plan.</Empty>
        )
      ) : null}
    </section>
  );
}
