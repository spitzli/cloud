import { useState } from "react";
import { Link, useBlocker } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useApi, useSession } from "../context";
import { Empty, Failure, Loading, Modal } from "../common";
import { ActionFailure } from "../HostingActionState";
import { uncertainOutcome } from "../hosting-client";
import { InstallationCatalog } from "./InstallationCatalog";
import * as ui from "../ui";
import {
  availability,
  blockerText,
  extensionReceipt,
  installedSchema,
  stateRequest,
  type Installation,
  type StateRequest,
} from "./lifecycle-contracts";
export function InstalledExtensions() {
  const api = useApi();
  const session = useSession();
  const allowed = session.user.role === "operator" && !session.user.preview;
  const query = useQuery({
    queryKey: ["installed-extensions"],
    queryFn: ({ signal }) =>
      api.get("/api/console/plugins/installed", installedSchema, signal),
    enabled: allowed,
  });
  const [selected, setSelected] = useState<Installation>();
  if (!allowed)
    return (
      <Empty title="Operator access required">
        Installation availability is managed by an operator outside customer
        preview.
      </Empty>
    );
  if (query.isPending) return <Loading>Loading installed extensions…</Loading>;
  if (query.error)
    return <Failure error={query.error} retry={() => void query.refetch()} />;
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>Installed extensions</h1>
          <p className={ui.pageLead}>
            Review availability, runtime state and dependencies.
          </p>
        </div>
        <button
          className={ui.button()}
          onClick={() => void query.refetch()}
          disabled={query.isFetching}
        >
          Refresh
        </button>
      </div>
      <p className={ui.cx(`${ui.notice} mb-4 max-w-none text-muted`)}>
        Disabling stops new authorized work and may wait for accepted work to
        drain. The current controller retains deployed processes and data; this
        is not an uninstall or process shutdown.
      </p>
      {query.data.length === 0 ? (
        <Empty title="No installation records">
          No extensions have been installed. Choose a reviewed release from the catalog below.
        </Empty>
      ) : (
        <div className={ui.tableFrame}>
          <table className={ui.table}>
            <thead>
              <tr>
                <th scope="col" className={ui.cx(`${ui.th} w-1/2`)}>Extension</th>
                <th scope="col" className={ui.th}>Availability</th>
                <th scope="col" className={ui.th}>Runtime</th>
                <th scope="col" className={ui.th}>Actions</th>
              </tr>
            </thead>
            <tbody>
              {query.data.map((plugin) => (
                <tr key={plugin.id} className={ui.tr}>
                  <td className={ui.cx(`${ui.td} min-w-[170px]`)}>
                    {plugin.kind === "external" && plugin.runtime === "ready" ? (
                      <Link className="font-medium underline-offset-[3px] hover:underline" to="/extensions/$plugin" params={{ plugin: plugin.id }}>
                        {plugin.id}
                      </Link>
                    ) : (
                      <strong className="font-medium">{plugin.id}</strong>
                    )}
                    <span className={ui.cx(`block text-muted ${ui.mono}`)}>
                      {plugin.kind === "builtin" ? "Built-in" : "External"} ·{" "}
                      {plugin.version}
                    </span>
                    {plugin.dependencies.length ? (
                      <p className="mt-1 text-[13px] text-muted">
                        Requires{" "}
                        {plugin.dependencies
                          .map((value) => value.id)
                          .join(", ")}
                      </p>
                    ) : null}
                    {plugin.blockers.map((blocker, index) => (
                      <p key={index} className="mt-1 text-[13px] text-danger">
                        {blockerText(blocker)}
                      </p>
                    ))}
                  </td>
                  <td className={ui.td}>
                    <span className={ui.cx(`${ui.badge(ui.statusTone(plugin.state))} normal-case`)}>{plugin.state}</span>
                    <span className="mt-1 block text-xs text-muted">
                      Requested: {plugin.desiredState}
                    </span>
                  </td>
                  <td className={ui.td}>
                    <span className={ui.cx(`${ui.badge(ui.statusTone(plugin.runtime))} normal-case`)}>{plugin.runtime}</span>
                  </td>
                  <td className={ui.cx(`${ui.td} whitespace-nowrap`)}>
                    {plugin.operationID ? <Link className={ui.cx(ui.button("quiet"), "mr-2")} to="/extensions/operations/$operationID" params={{ operationID: plugin.operationID }}>View operation</Link> : null}
                    {plugin.managed && !plugin.installRecovery ? <Link className={ui.cx(ui.button("quiet"), "mr-2")} to="/extensions/$plugin/releases" params={{ plugin: plugin.id }}>Versions</Link> : null}
                    {!plugin.installRecovery ? <button
                      className={ui.button()}
                      disabled={plugin.runtime === "starting"}
                      onClick={() => {
                        setSelected(plugin);
                      }}
                    >
                      Change availability
                    </button> : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <InstallationCatalog installed={query.data.map((plugin) => plugin.id)} />
      {selected ? (
        <ExtensionStateForm
          key={`${selected.id}:${selected.revision}`}
          plugin={selected}
          close={() => {
            setSelected(undefined);
          }}
        />
      ) : null}
    </section>
  );
}
function ExtensionStateForm({
  plugin,
  close,
}: {
  plugin: Installation;
  close: () => void;
}) {
  const api = useApi();
  const client = useQueryClient();
  const mutation = useMutation({
    mutationFn: (request: StateRequest) =>
      api.write(
        "/api/console/plugins/installed",
        "POST",
        request,
        extensionReceipt,
      ),
    retry: false,
    gcTime: 0,
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["installed-extensions"] });
    },
  });
  const uncertain = uncertainOutcome(mutation.error);
  const [confirmed, setConfirmed] = useState(false);
  useBlocker({
    shouldBlockFn: () =>
      uncertain &&
      !window.confirm(
        "This request's outcome is unknown. Leave and review the installation state?",
      ),
    enableBeforeUnload: uncertain,
  });
  const dismiss = () => {
    if (mutation.isPending) return;
    if (
      uncertain &&
      !window.confirm(
        "The outcome is unknown. Close and refresh the installation state before another request?",
      )
    )
      return;
    close();
  };
  return (
    <Modal title={`Availability: ${plugin.id}`} close={dismiss}>
      {mutation.data ? (
        <div role="status" className="grid gap-2 text-sm">
          <h3 className="font-semibold">Request recorded: {mutation.data.status}</h3>
          <p className="text-muted">
            Availability changes only after the controller processes this
            operation.
          </p>
          <Link
            className={ui.cx(`${ui.button()} mt-2 justify-self-start`)}
            to="/extensions/operations/$operationID"
            params={{ operationID: mutation.data.id }}
          >
            View operation
          </Link>
        </div>
      ) : (
        <form
          className="grid gap-5 text-sm"
          onSubmit={(event) => {
            event.preventDefault();
            const data = new FormData(event.currentTarget);
            mutation.mutate(
              stateRequest(
                plugin.id,
                plugin.revision,
                availability.parse(data.get("desired")),
                crypto.randomUUID(),
              ),
            );
          }}
        >
          <p>
            Reviewed revision {plugin.revision}. Current availability:{" "}
            {plugin.state}; requested: {plugin.desiredState}; runtime:{" "}
            {plugin.runtime}.
          </p>
          <label className={ui.field}>
            Requested availability
            <select
              className={ui.input}
              name="desired"
              aria-label="Requested availability"
              defaultValue={plugin.desiredState}
              disabled={mutation.isPending || uncertain}
            >
              <option value="enabled">Enabled</option>
              <option value="draining">Draining</option>
              <option value="disabled">Disabled</option>
            </select>
          </label>
          <p className="text-muted">
            Disabling retains deployed processes and stored data. Dependencies
            or accepted work can block the transition.
          </p>
          <label className="flex items-center gap-2 text-[13px]">
            <input
              type="checkbox"
              className="size-4 accent-accent"
              checked={confirmed}
              onChange={(event) => {
                setConfirmed(event.target.checked);
              }}
              disabled={mutation.isPending || uncertain}
            />
            I reviewed the requested availability change.
          </label>
          <button
            className={ui.cx(`${ui.button("primary")} justify-self-start`)}
            disabled={!confirmed || mutation.isPending || uncertain}
          >
            {mutation.isPending ? "Requesting…" : "Request availability change"}
          </button>
        </form>
      )}
      <ActionFailure
        error={mutation.error}
        uncertain={uncertain}
        pending={mutation.isPending}
        retry={() => {
          if (mutation.variables) mutation.mutate(mutation.variables);
        }}
      />
    </Modal>
  );
}
