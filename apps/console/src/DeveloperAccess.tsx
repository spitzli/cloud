import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useApi } from "./context";
import { tenantsSchema } from "./workspace-contracts";
import { Failure, Loading } from "./common";
import { WorkspaceAvatar } from "./WorkspaceAvatar";
import * as ui from "./ui";

/** Personal sign-in delegates MCP access; automation keys belong to a workspace. */
export function DeveloperAccess() {
  const api = useApi();
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState(false);
  const workspaces = useQuery({
    queryKey: ["account-tenants"],
    queryFn: ({ signal }) => api.request("/api/tenants", tenantsSchema, { signal }, true),
  });
  const endpoint = api.config.consoleOrigin ? new URL("/api/mcp", api.config.consoleOrigin).href : null;
  const copyServerUrl = async () => {
    if (!endpoint) return;
    setCopied(false); setCopyError(false);
    try { await navigator.clipboard.writeText(endpoint); setCopied(true); }
    catch { setCopyError(true); }
  };
  const managers = workspaces.data?.filter((workspace) => workspace.role === "owner" || workspace.role === "admin");
  return <section className="grid gap-6">
    <div><h1 className={ui.pageTitle}>Developer access</h1><p className={ui.pageLead}>Connect your tools with your account. Keep automation scoped to a workspace.</p></div>
    <section className={ui.card} aria-labelledby="mcp-heading">
      <div className={ui.cardBody}>
        <h2 id="mcp-heading" className={ui.sectionTitle}>MCP connections</h2>
        <p className="text-sm text-muted">Have an operator register your MCP client’s exact callback URL and give you its client ID. Configure that ID with this server URL, then sign in with your Spitzli account and review the requested permissions.</p>
        {endpoint ? <>
          <label className={ui.field}>Server URL<input className={ui.input} value={endpoint} readOnly onFocus={(event) => { event.currentTarget.select(); }} /></label>
          <div className="flex flex-wrap items-center gap-3"><button type="button" className={ui.button()} onClick={() => { void copyServerUrl(); }}>Copy server URL</button><span role="status" className="text-sm text-muted">{copied ? "Server URL copied." : "Sign-in method: OAuth"}</span></div>
          {copyError ? <p role="alert" className="text-sm text-danger">Copy is unavailable. Select the server URL and copy it manually.</p> : null}
        </> : <p className="text-sm text-muted">The MCP server URL is unavailable. Open Console to check your connection.</p>}
        <a className="w-fit text-sm underline underline-offset-4" href="https://docs.spitzli.cloud/developers/mcp" target="_blank" rel="noreferrer">MCP setup guide</a>
        <p className="text-xs text-muted">Access follows your current workspace role. Customer preview cannot grant access. Workspace API tokens are for the REST API; they do not replace MCP authorization.</p>
      </div>
    </section>
    <section className={ui.card} aria-labelledby="keys-heading">
      <div className={ui.cardBody}><h2 id="keys-heading" className={ui.sectionTitle}>Workspace API tokens</h2><p className="text-sm text-muted">Create a separate token for each script or CI pipeline. Choose read-only access when possible, set an expiry, and revoke tokens you no longer need. Only workspace owners and administrators can manage them.</p></div>
      {workspaces.isPending ? <div className={ui.cardBody}><Loading>Loading your workspaces…</Loading></div> : null}
      {workspaces.isError ? <div className={ui.cardBody}><Failure error={workspaces.error} retry={() => void workspaces.refetch()} /></div> : null}
      {managers?.map((workspace) => <div className={ui.cardRow} key={workspace.id}>
        <div className="flex min-w-0 items-center gap-3"><WorkspaceAvatar name={workspace.name} icon={workspace.icon} /><span className="min-w-0 [overflow-wrap:anywhere]">{workspace.name}</span></div>
        {api.config.consoleOrigin ? <a className={ui.button()} href={`${api.config.consoleOrigin}/api/sso/login?returnTo=${encodeURIComponent(`/workspaces/${workspace.id}/api-tokens`)}`}>Manage tokens</a> : null}
      </div>)}
      {managers?.length === 0 ? <p className={ui.cx(ui.cardBody, "text-sm text-muted")}>You do not manage API tokens in any workspace. Ask a workspace owner or administrator.</p> : null}
    </section>
  </section>;
}
