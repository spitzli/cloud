import { useEffect, useState } from "react";
import { Link, useBlocker, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useApi, useSession } from "../context";
import { Empty, Failure, Loading, Modal } from "../common";
import { ActionFailure } from "../HostingActionState";
import { uncertainOutcome } from "../hosting-client";
import * as ui from "../ui";
import { useInstallationCatalog } from "./InstallationCatalog";
import { type CatalogRelease, installationPhase } from "./installation-contracts";
import { extensionOperation, extensionPolling, extensionReceipt, installedSchema, stateRequest, type Installation, type StateRequest } from "./lifecycle-contracts";
import { readsReservedSchema, releaseHistorySchema, rollbackRequestSchema, updateRequestSchema, type ReleaseHistory, type VersionCommand } from "./version-contracts";

type Choice = { kind: "update"; release: CatalogRelease } | { kind: "rollback"; release: CatalogRelease; generation: string };
export function ExtensionVersions({ plugin }: { plugin: string }) {
  const api = useApi(); const session = useSession(); const client = useQueryClient();
  const allowed = session.user.role === "operator" && !session.user.preview;
  const [choice, setChoice] = useState<Choice>();
  const [confirmedDisable, setConfirmedDisable] = useState(false);
  const [requestedOperation, setRequestedOperation] = useState<string>();
  const disable = useMutation({
    mutationFn: (request: StateRequest) => api.write("/api/console/plugins/installed", "POST", request, extensionReceipt), retry: false,
    onSuccess: (receipt) => { setRequestedOperation(receipt.id); void client.invalidateQueries({ queryKey: ["installed-extensions"] }); },
  });
  const disableUncertain = uncertainOutcome(disable.error);
  useBlocker({ shouldBlockFn: () => disableUncertain && !window.confirm("The disable outcome is unknown. Leave and review its server state?"), enableBeforeUnload: disableUncertain });
  const installed = useQuery({ queryKey: ["installed-extensions"], queryFn: ({ signal }) => api.get("/api/console/plugins/installed", installedSchema, signal), enabled: allowed, refetchInterval: disableUncertain ? 2000 : false });
  const history = useQuery({ queryKey: ["extension-history", plugin], queryFn: ({ signal }) => api.get(`/api/console/plugins/${encodeURIComponent(plugin)}/releases`, releaseHistorySchema, signal), enabled: allowed });
  const catalog = useInstallationCatalog(allowed);
  const installation = installed.data?.find((value) => value.id === plugin);
  const operationID = requestedOperation ?? installation?.operationID;
  const operation = useQuery({ queryKey: ["extension-operation", operationID], queryFn: ({ signal }) => api.get(`/api/console/plugins/operations/${operationID ?? ""}`, extensionOperation, signal), enabled: allowed && Boolean(operationID), refetchInterval: (value) => extensionPolling(value.state.data?.status, value.state.status === "error") });
  const status = operation.data?.status;
  useEffect(() => {
    if (status && !extensionPolling(status, false)) {
      void client.invalidateQueries({ queryKey: ["installed-extensions"] });
      void client.invalidateQueries({ queryKey: ["extension-history", plugin] });
    }
  }, [client, plugin, operationID, status]);
  if (!allowed) return <Empty title="Operator access required">Version changes are unavailable in this account context.</Empty>;
  if (!installed.data || !history.data || !catalog.data) {
    const error = installed.error ?? history.error ?? catalog.error;
    return error ? <Failure error={error} retry={() => { void installed.refetch(); void history.refetch(); void catalog.refetch(); if (operationID) void operation.refetch(); }} /> : <Loading>Loading release history…</Loading>;
  }
  if (!installation?.managed) return <Empty title="Managed installation required">This extension has no native managed release history.</Empty>;
  const retained = history.data;
  const busy = status === "pending" || status === "running";
  const canChange = installation.state === "disabled" && !busy && !disableUncertain && !disable.isPending;
  const available = catalog.data.filter((release) => release.pluginID === plugin && release.version !== installation.version);
  return <section>
    <div className={ui.pageHeading}><div><Link className={ui.backLink} to="/extensions">Extensions</Link><h1 className={ui.pageTitle}>{plugin} versions</h1><p className={ui.pageLead}>Review signed versions and retain existing data when changing the runtime.</p></div><button className={ui.button()} onClick={() => { void installed.refetch(); void history.refetch(); void catalog.refetch(); if (operationID) void operation.refetch(); }}>Refresh</button></div>
    <div className={ui.cx(ui.card, "mb-5")}><dl className={ui.cx(ui.cardBody, "sm:grid-cols-3")}>
      <div><dt className="text-xs text-muted">Current version</dt><dd className="mt-1 text-sm">{installation.version}</dd></div>
      <div><dt className="text-xs text-muted">Runtime generation</dt><dd className="mt-1 text-sm">{retained.currentGeneration ?? "Not promoted"}</dd></div>
      <div><dt className="text-xs text-muted">Retained schema ceiling</dt><dd className="mt-1 text-sm">{retained.schemaHighWater ?? "Unavailable"}</dd></div>
    </dl></div>
    {installation.state !== "disabled" ? <form className={ui.cx(ui.card, ui.cardBody, "mb-5")} onSubmit={(event) => { event.preventDefault(); if (confirmedDisable) disable.mutate(stateRequest(plugin, installation.revision, "disabled", crypto.randomUUID())); }}>
      <h2 className={ui.sectionTitle}>Disable and drain first</h2><p className="text-sm text-muted">New customer actions stop while accepted work finishes. Stored data is retained.</p>
      <label className="flex items-start gap-2 text-sm"><input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={confirmedDisable} disabled={disable.isPending || disableUncertain || busy} onChange={(event) => { setConfirmedDisable(event.target.checked); }} />Disable this extension before changing its version.</label>
      <button className={ui.cx(ui.button(), "justify-self-start")} disabled={!confirmedDisable || disable.isPending || disableUncertain || busy}>Disable for version change</button>
      <ActionFailure error={disable.error} uncertain={disableUncertain} pending={disable.isPending} retry={() => { if (disable.variables) disable.mutate(disable.variables); }} />
    </form> : <p className={ui.cx(ui.notice, "mb-5 max-w-none")}>Customer actions are disabled. Version changes retain the databases and their passwords.</p>}
    {operationID ? <p className="mb-5 text-sm" role="status"><span>{busy ? operation.data?.phase === "draining" ? "Waiting for accepted work" : installationPhase(operation.data?.phase ?? "requested") : "Latest operation"}</span> · <Link className="underline" to="/extensions/operations/$operationID" params={{ operationID }}>View operation</Link></p> : null}
    {operation.error ? <div className="mb-5"><Failure error={operation.error} retry={() => { void operation.refetch(); }} /></div> : null}
    <section aria-label="Available versions" className="mb-7"><h2 className={ui.cx(ui.sectionTitle, "mb-3")}>Available versions</h2>
      {available.length ? <ul className={ui.card}>{available.map((release) => {
        const compatible = readsReservedSchema(release, retained.schemaHighWater);
        const target = release.targets.find((value) => value.id === retained.targetID);
        return <li className={ui.cardRow} key={`${release.version}:${release.manifestSha256}`}><div className="min-w-0 flex-1"><strong className="text-sm">{release.version}</strong><p className="text-xs text-muted">Signed by {release.signerKeyID}</p>{!compatible ? <p className="mt-1 text-sm text-danger">This release cannot read the retained schema.</p> : !target ? <p className="mt-1 text-sm text-danger">The retained target is unavailable for this release.</p> : null}</div><button className={ui.button()} disabled={!canChange || !compatible || !target} onClick={() => { setChoice({ kind: "update", release }); }}>Review update to {release.version}</button></li>;
      })}</ul> : <Empty title="No other catalog versions">Only the current release is available in the trusted catalog.</Empty>}
    </section>
    <section aria-label="Release history"><h2 className={ui.cx(ui.sectionTitle, "mb-3")}>Release history</h2><ul className={ui.card}>{retained.releases.map((entry) => {
      const release = catalog.data.find((value) => value.pluginID === plugin && value.version === entry.version && value.manifestSha256 === entry.manifestSha256);
      const current = entry.generation === retained.currentGeneration;
      return <li key={entry.generation} className={ui.cx(ui.cardRow, "flex-wrap")}><div className="min-w-0 flex-1"><strong className="text-sm">{entry.version}</strong><p className="text-xs text-muted">Generation {entry.generation} · {current ? "Current" : entry.status}</p>{entry.rollbackBlocker ? <p className="mt-1 text-sm text-danger">{entry.rollbackBlocker === "schema_incompatible" ? "This release cannot read the retained schema." : entry.rollbackBlocker.replaceAll("_", " ")}</p> : null}</div>
        {!current || retained.candidateGeneration ? <button className={ui.button()} disabled={!canChange || !entry.rollbackEligible || !release?.targets.some((target) => target.id === retained.targetID)} onClick={() => { if (release) setChoice({ kind: "rollback", release, generation: entry.generation }); }}>Review rollback to {entry.version}</button> : null}
      </li>;
    })}</ul></section>
    {choice ? <VersionReview key={`${plugin}:${choice.kind}:${choice.release.manifestSha256}:${choice.kind === "rollback" ? choice.generation : ""}`} installation={installation} history={retained} choice={choice} close={() => { setChoice(undefined); }} /> : null}
  </section>;
}

function VersionReview({ installation, history, choice, close }: { installation: Installation; history: ReleaseHistory; choice: Choice; close: () => void }) {
  const api = useApi(); const client = useQueryClient(); const navigate = useNavigate();
  const [reviewed] = useState({ revision: installation.revision, version: installation.version });
  const [confirmed, setConfirmed] = useState(false);
  const mutation = useMutation({ mutationFn: (command: VersionCommand) => api.write(`/api/console/plugins/${installation.id}/${command.kind}`, "POST", command.body, extensionReceipt), retry: false,
    onSuccess: (receipt) => { void client.invalidateQueries({ queryKey: ["installed-extensions"] }); void navigate({ to: "/extensions/operations/$operationID", params: { operationID: receipt.id } }); } });
  const uncertain = uncertainOutcome(mutation.error); const locked = mutation.isPending || uncertain;
  useBlocker({ shouldBlockFn: () => uncertain && !window.confirm("The version change outcome is unknown. Leave and review its server state?"), enableBeforeUnload: uncertain });
  const target = choice.release.targets.find((value) => value.id === history.targetID);
  const compatible = readsReservedSchema(choice.release, history.schemaHighWater);
  const submit = () => {
    if (!confirmed || !target || !compatible || locked) return;
    const common = { expectedRevision: reviewed.revision, reviewSha256: target.reviewSha256, approvedCapabilities: choice.release.capabilities, idempotencyKey: crypto.randomUUID() };
    mutation.mutate(choice.kind === "update" ? { kind: "update", body: updateRequestSchema.parse({ ...common, version: choice.release.version }) } : { kind: "rollback", body: rollbackRequestSchema.parse({ ...common, releaseGeneration: choice.generation }) });
  };
  return <Modal title={`${choice.kind === "update" ? "Update" : "Roll back"} ${installation.id} to ${choice.release.version}`} close={close} dismissible={!locked}>
    <form className="grid gap-4" onSubmit={(event) => { event.preventDefault(); submit(); }}>
      <p className="text-sm text-muted">{reviewed.version} → {choice.release.version}. Existing databases and passwords are retained. Rollback starts a new runtime generation; it does not undo schema changes.</p>
      <dl className="grid gap-3 text-sm">{[["Signing key", choice.release.signerKeyID], ["Manifest SHA-256", choice.release.manifestSha256], ["Runtime image", choice.release.image], ["Target", target?.label ?? "Unavailable"], ["Permissions", choice.release.capabilities.join(", ") || "No additional host permissions"], ["Readable schema", choice.release.readableSchema ? `${String(choice.release.readableSchema.min)}–${String(choice.release.readableSchema.max)}` : "Unavailable"]].map(([label, value]) => <div key={label} className="min-w-0"><dt className="text-xs text-muted">{label}</dt><dd className="mt-1 [overflow-wrap:anywhere]">{value}</dd></div>)}</dl>
      {!compatible ? <p role="alert" className="text-sm text-danger">This release cannot read the retained schema.</p> : null}
      <label className="flex items-start gap-2 text-sm"><input type="checkbox" className="mt-0.5 size-4 shrink-0 accent-accent" checked={confirmed} disabled={locked} onChange={(event) => { setConfirmed(event.target.checked); }} />I reviewed this release, its permissions and schema compatibility.</label>
      <button className={ui.cx(ui.button("primary"), "justify-self-start")} disabled={!confirmed || !compatible || !target || locked}>{mutation.isPending ? "Recording version change…" : choice.kind === "update" ? "Update extension" : "Roll back extension"}</button>
      <ActionFailure error={mutation.error} uncertain={uncertain} pending={mutation.isPending} retry={() => { if (mutation.variables) mutation.mutate(mutation.variables); }} />
    </form>
  </Modal>;
}
