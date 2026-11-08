import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { commandSchema } from "@webdock/hosting-contracts";
import { type HostingApp, operationPolling, scalingReadSchema } from "./hosting-contracts";
import { useHostingAction } from "./hosting-client";
import { useApi } from "./context";
import { Failure, Loading } from "./common";
import { ActionFailure } from "./HostingActionState";
import * as ui from "./ui";

export function HostingScaling({ spec, available = false }: {
  spec?: HostingApp["spec"] | undefined;
  available?: boolean | undefined;
}) {
  const [mode, setMode] = useState(spec?.autoscaling ? "automatic" : "fixed");
  const blocked = !available || Boolean(spec?.volumeBytes);
  return <div className="grid gap-4 border-t border-outline pt-5">
    <label className={ui.field}>Scaling mode
      <select className={ui.input} name="scalingMode" value={mode} onChange={e => { setMode(e.target.value); }}>
        <option value="fixed">Fixed instances</option>
        <option value="automatic" disabled={blocked}>Automatic scaling</option>
      </select>
    </label>
    {mode === "fixed" ? <label className={ui.field}>Instances
      <input className={ui.input} type="number" name="replicas" min={0} max={20} step={1} required defaultValue={spec?.replicas ?? 1} />
      <span className={ui.hint}>Choose a fixed count. Zero stops the application.</span>
    </label> : <>
      <div className="grid gap-4 sm:grid-cols-2">
        <label className={ui.field}>Minimum instances
          <input className={ui.input} name="minReplicas" type="number" min={1} max={20} step={1} required defaultValue={spec?.autoscaling?.minReplicas ?? 1} />
        </label>
        <label className={ui.field}>Maximum instances
          <input className={ui.input} name="maxReplicas" type="number" min={1} max={20} step={1} required defaultValue={spec?.autoscaling?.maxReplicas ?? 2} />
        </label>
      </div>
      <label className={ui.field}>Target CPU utilization (%)
        <input className={ui.input} name="targetCPUUtilizationPercent" type="number" min={10} max={90} step={1} required defaultValue={spec?.autoscaling?.targetCPUUtilizationPercent ?? 70} />
        <span className={ui.hint}>Relative to each instance’s configured CPU. Scaling down stabilizes for five minutes.</span>
      </label>
      <p className={ui.hint}>Capacity for the maximum count is reserved against your plan. This is not the number of instances currently running. A stopped application stays stopped when you save.</p>
    </>}
    {blocked ? <p className={ui.hint}>{spec?.volumeBytes ? "Apps with local persistent storage support one fixed instance." : "Automatic scaling is not available on this cluster yet."}</p> : null}
  </div>;
}

export function HostingScalingStatus({ app }: { app: HostingApp }) {
  const { mutation, retry, uncertain } = useHostingAction();
  const [complete, setComplete] = useState(false);
  return <section className={ui.cx(`${ui.card} mt-6`)} aria-label="Automatic scaling">
    <div className={ui.cx(`${ui.cardBody} gap-4`)}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div><h2 className={ui.sectionTitle}>Automatic scaling</h2>
          <p className={ui.hint}>Current instances are a runtime snapshot. Reserved maximum capacity is shown in Usage.</p></div>
        <button className={ui.button()} disabled={!app.canRequestLogs || mutation.isPending || uncertain || (Boolean(mutation.data) && !complete)} onClick={() => {
          setComplete(false);
          mutation.mutate(commandSchema.parse({ action: "apps.inspect", appID: app.id, idempotencyKey: crypto.randomUUID() }));
        }}>Refresh live instances</button>
      </div>
      <ActionFailure error={mutation.error} uncertain={uncertain} retry={retry} pending={mutation.isPending} />
      {mutation.data ? <ScalingSnapshot key={mutation.data.id} id={mutation.data.id} onTerminal={setComplete} /> : <p className={ui.hint}>No live snapshot requested.</p>}
    </div>
  </section>;
}

function ScalingSnapshot({ id, onTerminal }: { id: string; onTerminal: (value: boolean) => void }) {
  const api = useApi();
  const query = useQuery({
    queryKey: ["hosting-scaling-read", id],
    queryFn: ({ signal }) => api.get(`/api/console/hosting/reads/${encodeURIComponent(id)}`, scalingReadSchema, signal),
    refetchInterval: value => operationPolling(value.state.data?.status, value.state.status === "error"),
  });
  const status = query.data?.status;
  useEffect(() => { if (status && !operationPolling(status, false)) onTerminal(true); }, [status, onTerminal]);
  if (query.isPending) return <Loading>Reading current instances…</Loading>;
  if (query.error) return <Failure error={query.error} retry={() => void query.refetch()} />;
  if (query.data.error) return <p role="alert" className="text-danger">{query.data.error}</p>;
  const result = query.data.result;
  const state = result?.autoscaling;
  if (!result || !state) return <p role="status" className={ui.hint}>{status === "succeeded" ? "Automatic scaling is stopped or no longer active." : `Instance read: ${status ?? "pending"}`}</p>;
  return <>
    <dl className="grid grid-cols-3 gap-4">
      {[["Current", state.currentReplicas], ["Ready", state.readyReplicas], ["Desired", state.desiredReplicas]].map(([label, value]) => <div key={label}><dt className={ui.hint}>{label}</dt><dd className="mt-1 text-lg font-medium">{value}</dd></div>)}
    </dl>
    <p role="status" className={ui.hint}>{state.metricsAvailable ? "CPU metrics available." : "CPU metrics are temporarily unavailable. A new scaling target cannot be calculated yet."}</p>
    <p className={ui.hint}>Observed {new Date(result.observedAt * 1000).toLocaleString()}</p>
  </>;
}
