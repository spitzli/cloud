import { useEffect } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useApi, useSession } from "../context";
import { Empty, Failure, Loading } from "../common";
import { extensionOperation, extensionPolling, extensionReceipt } from "./lifecycle-contracts";
import { installationPhase } from "./installation-contracts";
import { ActionFailure } from "../HostingActionState";
import { uncertainOutcome } from "../hosting-client";
import { backLink } from "../ui";
import * as ui from "../ui";
export function ExtensionOperation({ id }: { id: string }) {
  const api = useApi();
  const session = useSession();
  const client = useQueryClient();
  const allowed = session.user.role === "operator" && !session.user.preview;
  const query = useQuery({
    queryKey: ["extension-operation", id],
    queryFn: ({ signal }) =>
      api.get(
        `/api/console/plugins/operations/${encodeURIComponent(id)}`,
        extensionOperation,
        signal,
      ),
    enabled: allowed,
    refetchInterval: (value) =>
      extensionPolling(
        value.state.data?.status,
        value.state.status === "error",
      ),
  });
  const status = query.data?.status;
  useEffect(() => {
    if (status && !extensionPolling(status, false))
      void client.invalidateQueries({ queryKey: ["installed-extensions"] });
  }, [client, id, status]);
  if (!allowed)
    return (
      <Empty title="Operator access required">
        Operation details are unavailable in this account context.
      </Empty>
    );
  if (query.isPending) return <Loading>Loading extension operation…</Loading>;
  if (query.error)
    return <Failure error={query.error} retry={() => void query.refetch()} />;
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <Link className={backLink} to="/extensions">
            Installed extensions
          </Link>
          <h1 className={ui.pageTitle}>{{ install: "Extension installation", update: "Extension update", rollback: "Extension rollback", state: "Extension operation" }[query.data.kind]}</h1>
          <p className={ui.pageLead}>
            {query.data.pluginID} · <span className={ui.mono}>{query.data.id}</span>
          </p>
        </div>
        <button
          className={ui.button()}
          disabled={query.isFetching}
          onClick={() => void query.refetch()}
        >
          Refresh
        </button>
      </div>
      <div className={ui.card}>
      <dl className={ui.cx(`${ui.cardBody} sm:grid-cols-3`)}>
        <div>
          <dt className="text-xs text-muted">Status</dt>
          <dd className="mt-1 text-sm font-medium" role="status">{query.data.status}</dd>
        </div>
        <div>
          <dt className="text-xs text-muted">Phase</dt>
          <dd className="mt-1 text-sm">{query.data.kind !== "state" ? installationPhase(query.data.phase) : query.data.phase}</dd>
        </div>
        <div>
          <dt className="text-xs text-muted">Generation</dt>
          <dd className="mt-1 text-sm">{query.data.generation}</dd>
        </div>
      </dl>
      {query.data.condition ? (
        <p
          role={
            query.data.status === "blocked" || query.data.status === "failed"
              ? "alert"
              : "status"
          }
          className={ui.cx(`${ui.cardRow} block`)}
        >
          {query.data.condition}
        </p>
      ) : null}
      <p className={ui.cardFooter}>{query.data.kind === "install" ? installationDescription(query.data.status) : query.data.kind === "state" ? extensionDescription(query.data.status) : versionDescription(query.data.status)}</p>
      </div>
      {query.data.kind === "install" && query.data.status === "succeeded" ? <Link className={ui.cx(ui.button("primary"), "mt-5")} to="/extensions/$plugin" params={{ plugin: query.data.pluginID }}>Configure self-service</Link> : null}
      {["update", "rollback"].includes(query.data.kind) ? <Link className={ui.cx(ui.button("quiet"), "mt-5 mr-3")} to="/extensions/$plugin/releases" params={{ plugin: query.data.pluginID }}>Version history</Link> : null}
      {query.data.kind !== "state" && ["blocked", "failed"].includes(query.data.status) ? <RetryInstallation key={id} id={id} kind={query.data.kind} /> : null}
    </section>
  );
}
function RetryInstallation({ id, kind }: { id: string; kind: "install" | "update" | "rollback" }) {
  const api = useApi();
  const navigate = useNavigate();
  const retry = useMutation({ mutationFn: (idempotencyKey: string) => api.write(`/api/console/plugins/operations/${id}/retry`, "POST", { idempotencyKey }, extensionReceipt),
    onSuccess: (receipt) => { void navigate({ to: "/extensions/operations/$operationID", params: { operationID: receipt.id } }); } });
  const uncertain = uncertainOutcome(retry.error);
  return <div className="mt-5 grid gap-3"><p className="text-sm text-muted">Retry retains this installation's approved release, private storage and identity.</p>
    <button className={ui.cx(ui.button(), "justify-self-start")} disabled={retry.isPending || uncertain} onClick={() => { retry.mutate(crypto.randomUUID()); }}>Retry {kind === "install" ? "installation" : kind}</button>
    <ActionFailure error={retry.error} uncertain={uncertain} pending={retry.isPending} retry={() => { if (retry.variables) retry.mutate(retry.variables); }} />
  </div>;
}
function versionDescription(status: string) {
  switch (status) {
    case "pending": return "The version request is saved. Waiting for the controller.";
    case "running": return "Previous runtimes must stop before the reviewed version starts. Customer access remains disabled until identity and readiness are verified.";
    case "succeeded": return "The reviewed version is active. Existing databases, passwords and stored data are retained.";
    case "blocked": case "failed": return "The version change needs attention. Resources and data are retained; review this condition before retrying or selecting a compatible rollback.";
    default: return "Version state is unavailable.";
  }
}
function installationDescription(status: string) {
  switch (status) {
    case "pending": return "The installation request is saved. Waiting for the controller.";
    case "running": return "Installation is in progress. Runtime access remains disabled until identity and readiness are verified.";
    case "succeeded": return "The runtime identity and readiness were verified. Configure which features customers may use next.";
    case "blocked": return "Installation needs attention. Review the reported condition before retrying; private storage is retained.";
    case "failed": return "Installation did not complete. Its private storage and identity are retained for recovery.";
    default: return "Installation state is unavailable.";
  }
}
function extensionDescription(status: string) {
  switch (status) {
    case "pending":
      return "Waiting for the controller. Requested availability has not been confirmed.";
    case "running":
      return "The controller is processing this availability change.";
    case "blocked":
      return "This operation is blocked. Review its condition and dependencies before requesting another change.";
    case "failed":
      return "The operation failed. Refresh the installation state before deciding what to do next.";
    case "succeeded":
      return "The controller recorded completion. Deployed processes and stored data are retained by availability changes.";
    default:
      return "Operation state is unavailable.";
  }
}
