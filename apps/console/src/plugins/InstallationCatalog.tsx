import { Link } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { useApi } from "../context";
import { Empty, Failure, Loading } from "../common";
import * as ui from "../ui";
import { catalogSchema } from "./installation-contracts";

export function useInstallationCatalog(enabled = true) {
  const api = useApi();
  return useQuery({ queryKey: ["extension-catalog"], queryFn: ({ signal }) => api.get("/api/console/plugins/catalog", catalogSchema, signal), enabled });
}
/** Only mounted inside the operator route; catalog metadata never grants runtime authority. */
export function InstallationCatalog({ installed = [] }: { installed?: string[] }) {
  const query = useInstallationCatalog();
  return <section className="mt-8" aria-label="Available extensions">
    <h2 className={ui.sectionTitle}>Available extensions</h2>
    <p className="mb-4 mt-1 text-sm text-muted">Reviewed releases from this installation's configured catalog.</p>
    {query.isPending ? <Loading>Loading available releases…</Loading> : query.error ? <Failure error={query.error} retry={() => { void query.refetch(); }} /> : query.data.length ? (
      <ul className={ui.card}>{query.data.map((release) => (
        <li key={`${release.catalogID}:${release.version}`} className={ui.cx(ui.cardRow, "first:border-t-0")}>
          <span className="grid min-w-0 gap-1"><strong className="font-medium">{release.pluginID} · {release.version}</strong><span className="text-xs text-muted">Signed by {release.signerKeyID}</span></span>
          {installed.includes(release.pluginID) ? <span className="text-sm text-muted">Installation recorded above</span> : <Link className={ui.button()} to="/extensions/install/$catalogID/$version" params={{ catalogID: release.catalogID, version: release.version }} aria-label={`Review ${release.pluginID} ${release.version}`}>Review release</Link>}
        </li>
      ))}</ul>
    ) : <Empty title="No catalog releases">Configure a reviewed release and an enrolled installation target to install an extension.</Empty>}
  </section>;
}
