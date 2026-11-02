import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useHostingCommand } from "./hosting-client";
import { publicAddressViewSchema } from "@webdock/hosting-contracts";
import { useState } from "react";
import { z } from "zod";
import { useApi, useSession } from "./context";
import { Failure, Loading } from "./common";
import { useTenants } from "./WorkspaceNav";
import * as ui from "./ui";

const domainSchema = z.object({
  id: z.string(),
  domain: z.string(),
  verified: z.boolean(),
  verifiedAt: z.string().nullable(),
  checkedAt: z.string().nullable(),
  record: z.object({ type: z.string(), name: z.string(), value: z.string() }),
  usedBy: z.array(z.object({ plugin: z.string(), unit: z.string(), project: z.string().nullable(), resource: z.string().nullable() })),
});
type Domain = z.infer<typeof domainSchema>;

function Record({ domain }: { domain: Domain }) {
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 rounded-lg border border-outline bg-subtle p-3 text-[13px]">
      <dt className="text-muted">Type</dt>
      <dd><code className={ui.mono}>{domain.record.type}</code></dd>
      <dt className="text-muted">Name</dt>
      <dd className="[overflow-wrap:anywhere]"><code className={ui.mono}>{domain.record.name}</code></dd>
      <dt className="text-muted">Value</dt>
      <dd className="[overflow-wrap:anywhere]"><code className={ui.mono}>{domain.record.value}</code></dd>
    </dl>
  );
}
function Row({ customer, domain, writable }: { customer: string; domain: Domain; writable: boolean }) {
  const api = useApi();
  const client = useQueryClient();
  const refresh = () => client.invalidateQueries({ queryKey: ["domains", customer] });
  const path = `/api/console/tenant/${customer}/domains/${encodeURIComponent(domain.domain)}`;
  const verify = useMutation({ mutationFn: () => api.write(path, "POST", {}, domainSchema), onSuccess: refresh });
  const remove = useMutation({ mutationFn: () => api.request(path, z.object({}), { method: "DELETE" }), onSuccess: refresh });
  const error = verify.error ?? remove.error;
  const checked = verify.data && !verify.data.verified;
  return (
    <li className="grid gap-3 border-t border-outline px-6 py-4 first:border-t-0 max-sm:px-[18px]">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-3">
          <span className="font-medium [overflow-wrap:anywhere]">{domain.domain}</span>
          <span className={ui.badge(domain.verified ? "success" : "warning")}>{domain.verified ? "Verified" : "Not verified"}</span>
        </div>
        {writable ? (
          <div className="flex gap-2">
            {!domain.verified ? (
              <button type="button" className={ui.button("primary")} disabled={verify.isPending} onClick={() => { verify.mutate(); }}>
                {verify.isPending ? "Checking…" : "Check record"}
              </button>
            ) : null}
            <button type="button" className={ui.button("quiet")} disabled={remove.isPending || domain.usedBy.length > 0} onClick={() => { remove.mutate(); }}>
              Remove
            </button>
          </div>
        ) : null}
      </div>
      {!domain.verified ? (
        <>
          <p className="text-[13px] text-muted">Add this record at your DNS provider, then check it. It can take a few minutes to show up.</p>
          <Record domain={domain} />
          {checked ? <p role="status" className="text-[13px] text-warning">The record is not there yet.</p> : null}
        </>
      ) : null}
      {domain.usedBy.length ? (
        <p className="text-[13px] text-muted">
          Used by {domain.usedBy.map((u) => `${u.resource ?? u.unit} (${u.project ?? "Workspace"})`).join(", ")}
        </p>
      ) : null}
      {error ? <p role="alert" className="text-sm text-danger">{error.message}</p> : null}
    </li>
  );
}
function AssignedAddresses({id}:{id:string}) {
  const call=useHostingCommand();
  const query=useQuery({queryKey:["hosting","addresses",id],queryFn:({signal})=>call({action:"addresses.list",customerID:id,page:1,limit:50},z.object({docs:z.array(publicAddressViewSchema.extend({projectName:z.string(),appName:z.string()})),totalDocs:z.number()}),signal)});
  if(query.isPending)return <Loading>Loading assigned addresses…</Loading>;
  if(query.error)return <Failure error={query.error} retry={()=>void query.refetch()}/>;
  if(!query.data.docs.length)return null;
  return <section className="mb-6" aria-label="Assigned addresses"><h2 className={ui.sectionTitle}>Assigned addresses</h2><ul className={ui.cx(ui.card,"mt-3")}>{query.data.docs.map(address=><li key={address.id} className="flex flex-wrap items-center justify-between gap-3 border-t border-outline px-6 py-4 first:border-t-0"><div className="min-w-0"><a className="break-all" href={address.url} target="_blank" rel="noopener noreferrer">{address.hostname}</a><p className="text-xs text-muted">{address.projectName} · {address.appName}</p></div><span className={ui.badge(address.state==="ready"?"success":"warning")}>{address.state==="ready"?"Ready":"Pending"}</span></li>)}</ul>{query.data.totalDocs>50?<p className="mt-2 text-xs text-muted">Open the project applications to see all assigned addresses.</p>:null}</section>;
}
/** A workspace's own domains: added, proven with a TXT record, then usable by its apps. */
export function WorkspaceDomains({ id }: { id: string }) {
  const api = useApi();
  const session = useSession();
  const tenants = useTenants();
  const client = useQueryClient();
  const [name, setName] = useState("");
  const domains = useQuery({
    queryKey: ["domains", id],
    queryFn: ({ signal }) => api.get(`/api/console/tenant/${id}/domains`, z.array(domainSchema), signal),
  });
  const add = useMutation({
    mutationFn: () => api.write(`/api/console/tenant/${id}/domains`, "POST", { domain: name }, domainSchema),
    onSuccess: () => {
      setName("");
      void client.invalidateQueries({ queryKey: ["domains", id] });
    },
  });
  const role = tenants.data?.find((t) => t.id === id)?.role;
  const writable = !(session.user.preview && !session.user.acting) && (session.user.role === "operator" || role === "owner" || role === "admin");
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>Domains</h1>
          <p className={ui.pageLead}>Prove a domain is yours once, then use it and its subdomains for your apps. Certificates are issued automatically.</p>
        </div>
      </div>
      {api.config.enabledFeatures.includes("hosting") ? <AssignedAddresses id={id}/>:null}
      {writable ? (
        <form
          className={ui.cx(ui.card, "mb-6")}
          onSubmit={(event) => {
            event.preventDefault();
            add.mutate();
          }}
        >
          <div className={ui.cx(ui.cardBody, "sm:grid-cols-[1fr_auto] sm:items-end")}>
            <label className={ui.field}>
              Domain
              <input className={ui.input} required maxLength={253} value={name} onChange={(e) => { setName(e.target.value); }} placeholder="example.com" autoComplete="off" spellCheck={false} />
            </label>
            <button className={ui.button("primary")} disabled={add.isPending}>Add domain</button>
          </div>
          {add.error ? <p role="alert" className="px-6 pb-4 text-sm text-danger">{add.error.message}</p> : null}
        </form>
      ) : null}
      {domains.isPending ? <Loading>Loading domains…</Loading> : null}
      {domains.isError ? <Failure error={domains.error} retry={() => void domains.refetch()} /> : null}
      {domains.data ? (
        domains.data.length ? (
          <ul className={ui.card}>
            {domains.data.map((d) => <Row key={d.id} customer={id} domain={d} writable={writable} />)}
          </ul>
        ) : (
          <p className="text-sm text-muted">No custom domains yet. Public apps can use an assigned address when public hosting is enabled.</p>
        )
      ) : null}
    </section>
  );
}
