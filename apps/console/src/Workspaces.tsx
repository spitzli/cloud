import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { Link } from "@tanstack/react-router";
import { useApi, useSession } from "./context";
import { Failure, Loading } from "./common";
import * as ui from "./ui";
import { ArrowLeftIcon } from "@phosphor-icons/react/dist/csr/ArrowLeft";
import { CustomerMembers } from "./CustomerMembers";
function useTenants() {
  const api = useApi();
  return useQuery({
    queryKey: ["tenants"],
    queryFn: () =>
      api.get(
        "/api/console/tenants",
        z.array(
          z.object({
            id: z.string(),
            name: z.string(),
            status: z.string(),
            role: z.string().nullable(),
          }),
        ),
      ),
  });
}
/** A customer's own members page: owners and admins invite people and owners hand on roles. */
export function WorkspaceMembers({ id }: { id: string }) {
  const tenants = useTenants();
  const name = tenants.data?.find((tenant) => tenant.id === id)?.name;
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <Link className={ui.backLink} to="/$collection" params={{ collection: "tenants" }}>
            <ArrowLeftIcon />
            Your workspaces
          </Link>
          <h1 className={ui.pageTitle}>{name ?? "Workspace members"}</h1>
          <p className={ui.pageLead}>Who can open this workspace and what they may change.</p>
        </div>
      </div>
      <CustomerMembers key={id} customer={id} />
    </section>
  );
}
export function Workspaces() {
  const api = useApi();
  const session = useSession();
  const client = useQueryClient();
  const tenants = useTenants();
  const exit = useMutation({
    mutationFn: () =>
      api.write("/api/console/tenant-preview/exit", "POST", {}, z.object({})),
    onSuccess: () => {
      client.clear();
      location.assign("/projects");
    },
  });
  if (tenants.isPending)
    return <Loading>Opening your workspaces…</Loading>;
  if (tenants.error)
    return (
      <Failure
        error={tenants.error}
        retry={() => {
          void tenants.refetch();
        }}
      />
    );
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>
            {session.user.preview ? session.user.acting ? "Acting as customer owner" : "Customer preview" : "Your workspaces"}
          </h1>
          <p className={ui.pageLead}>
            {session.user.preview
              ? session.user.acting ? "You can manage this customer with owner permissions." : "You are viewing this customer with read-only access."
              : "Organizations you belong to. Open one to see its projects, applications and databases."}
          </p>
        </div>
        {session.user.preview ? (
          <button
            className={ui.button()}
            onClick={() => {
              exit.mutate();
            }}
            disabled={exit.isPending}
          >
            Exit customer preview
          </button>
        ) : null}
      </div>
      {exit.error ? (
        <p className="mb-4 text-sm text-danger" role="alert">
          {exit.error.message}
        </p>
      ) : null}
      <section className={ui.card}>
        <h2 className={ui.cx(`${ui.sectionTitle} px-6 py-4 max-sm:px-[18px]`)}>Workspaces</h2>
        {tenants.data.length ? (
          <ul>
            {tenants.data.map((t) => (
              <li key={t.id} className={ui.cx(ui.cardRow, ui.clickableRow)}>
                <div>
                  <Link className={ui.cx(ui.rowLink, "font-medium")} to="/workspaces/$id" params={{ id: t.id }}>
                    {t.name}
                  </Link>
                  <p className="text-[13px] text-muted">
                    {t.status}
                    {t.role ? ` · ${t.role}` : ""}
                  </p>
                </div>
                <div className="flex flex-wrap gap-2">
                {!(session.user.preview && !session.user.acting) && (t.role === "owner" || t.role === "admin") ? (
                  <Link className={ui.cx(ui.button("quiet"), "relative z-10")} to="/$collection/$id" params={{ collection: "tenants", id: t.id }}>
                    Members
                  </Link>
                ) : null}
                {api.config.enabledFeatures.includes("mail") ? (
                  <Link
                    className={ui.cx(ui.button(), "relative z-10")}
                    to="/mail"
                    search={{ customer: t.id }}
                  >
                    Mail
                  </Link>
                ) : null}
                {api.config.enabledFeatures.includes("hosting") ? (
                  <Link
                    className={ui.cx(ui.button(), "relative z-10")}
                    to="/hosting"
                    search={{ customer: t.id }}
                  >
                    Applications
                  </Link>
                ) : null}
                </div>
              </li>
            ))}
          </ul>
        ) : (
          <p className={ui.cx(`${ui.cardRow} text-muted`)}>No customer workspace has been shared with this account.</p>
        )}
      </section>
    </section>
  );
}
