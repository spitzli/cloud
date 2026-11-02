import { useState } from "react";
import { Link, useBlocker, useNavigate } from "@tanstack/react-router";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useApi } from "../context";
import { Empty, Failure, Loading } from "../common";
import { ActionFailure } from "../HostingActionState";
import { uncertainOutcome } from "../hosting-client";
import * as ui from "../ui";
import { useInstallationCatalog } from "./InstallationCatalog";
import { type CatalogRelease, type InstallRequest, installRequestSchema } from "./installation-contracts";
import { extensionReceipt } from "./lifecycle-contracts";

export function InstallExtension({ catalogID, version }: { catalogID: string; version: string }) {
  const query = useInstallationCatalog();
  if (query.isPending) return <Loading>Loading reviewed release…</Loading>;
  if (query.error) return <Failure error={query.error} retry={() => { void query.refetch(); }} />;
  const release = query.data.find((item) => item.catalogID === catalogID && item.version === version);
  if (!release) return <Empty title="Release unavailable">Return to Extensions and choose a current reviewed release.</Empty>;
  return <InstallForm key={`${catalogID}:${version}:${release.manifestSha256}`} release={release} />;
}
function InstallForm({ release }: { release: CatalogRelease }) {
  const api = useApi();
  const navigate = useNavigate();
  const client = useQueryClient();
  const [target, setTarget] = useState(release.targets[0]?.id ?? "");
  const [configuration, setConfiguration] = useState("{}");
  const [confirmed, setConfirmed] = useState(false);
  const [error, setError] = useState("");
  const mutation = useMutation({
    mutationFn: (request: InstallRequest) => api.write("/api/console/plugins/install", "POST", request, extensionReceipt),
    retry: false,
    onSuccess: (receipt) => {
      void client.invalidateQueries({ queryKey: ["installed-extensions"] });
      void navigate({ to: "/extensions/operations/$operationID", params: { operationID: receipt.id } });
    },
  });
  const uncertain = uncertainOutcome(mutation.error);
  const locked = mutation.isPending || uncertain;
  useBlocker({ shouldBlockFn: () => uncertain && !window.confirm("The installation outcome is unknown. Leave and review its server state?"), enableBeforeUnload: uncertain });
  const configured = Object.keys((release.configurationSchema.properties as Record<string, unknown> | undefined) ?? {}).length > 0;
  const submit = () => {
    setError("");
    const selected = release.targets.find((item) => item.id === target);
    if (!selected || !confirmed) return;
    try {
      mutation.mutate(installRequestSchema.parse({ catalogID: release.catalogID, version: release.version,
        reviewSha256: selected.reviewSha256, targetID: selected.id, configuration: JSON.parse(configuration) as unknown,
        approvedCapabilities: release.capabilities, idempotencyKey: crypto.randomUUID() }));
    } catch { setError("Enter a valid configuration object."); }
  };
  return <section>
    <div className={ui.pageHeading}><div>
      <Link className={ui.backLink} to="/extensions">Extensions</Link>
      <h1 className={ui.pageTitle}>Install {release.pluginID}</h1>
      <p className={ui.pageLead}>Review this exact release and its permissions before installation.</p>
    </div></div>
    <div className={ui.cx(ui.card, "mb-6")}><dl className={ui.cx(ui.cardBody, "sm:grid-cols-2")}>
      {[ ["Version", release.version], ["Signing key", release.signerKeyID], ["Registry", release.registryURL], ["Manifest SHA-256", release.manifestSha256], ["Runtime image", release.image] ].map(([label, value]) => (
        <div key={label} className="min-w-0"><dt className="text-xs text-muted">{label}</dt><dd className="mt-1 text-sm [overflow-wrap:anywhere]">{value}</dd></div>
      ))}
      <div><dt className="text-xs text-muted">Permissions</dt><dd className="mt-1 text-sm">{release.capabilities.length ? release.capabilities.join(", ") : "No additional host permissions"}</dd></div>
    </dl></div>
    <form className={ui.cx(ui.card, ui.cardBody)} onSubmit={(event) => { event.preventDefault(); submit(); }}>
      <p className="text-sm text-muted">The controller prepares private storage and configuration, starts the runtime, then verifies its identity. Customer access is configured after installation.</p>
      <label className={ui.field}>Installation target<select className={ui.input} value={target} disabled={locked} onChange={(event) => { setTarget(event.target.value); setConfirmed(false); }}>
        {!release.targets.length ? <option value="">No eligible enrolled target</option> : null}
        {release.targets.map((item) => <option key={item.id} value={item.id}>{item.label}</option>)}
      </select></label>
      {configured ? <label className={ui.field}>Public configuration<textarea className={ui.input} value={configuration} disabled={locked} onChange={(event) => { setConfiguration(event.target.value); setConfirmed(false); }} /><span className="text-xs font-normal text-muted">Enter the release's configuration object. Credentials belong in private server setup.</span></label> : null}
      <label className="flex items-start gap-2 text-sm"><input className="mt-0.5 size-4 shrink-0 accent-accent" type="checkbox" checked={confirmed} disabled={locked} onChange={(event) => { setConfirmed(event.target.checked); }} />I reviewed this release, its permissions and the selected target.</label>
      {error ? <p role="alert" className="text-sm text-danger">{error}</p> : null}
      <button className={ui.cx(ui.button("primary"), "justify-self-start")} disabled={!confirmed || !target || locked}>{mutation.isPending ? "Recording installation…" : "Install extension"}</button>
      <ActionFailure error={mutation.error} uncertain={uncertain} pending={mutation.isPending} retry={() => { if (mutation.variables) mutation.mutate(mutation.variables); }} />
    </form>
  </section>;
}
