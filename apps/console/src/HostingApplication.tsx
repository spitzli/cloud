import { useState } from "react";
import { Link, useBlocker } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { commandSchema, formatResource } from "@spitzli/hosting-contracts";
import { useApi, useSession } from "./context";
import { Empty, Failure, Loading, Modal } from "./common";
import { useHostingAction, useHostingCommand } from "./hosting-client";
import {
  appSchema,
  actionCommand,
  canManageApp,
  type HostingApp,
  type LifecycleAction,
} from "./hosting-contracts";
import {
  HostingReceipt,
  HostingOperationLink,
  HostingLogRead,
  useHostingActionCompletion,
} from "./HostingOperation";
import { HostingSettings } from "./HostingSettings";
import { HostingScalingStatus } from "./HostingScaling";
import { HostingDeletion } from "./HostingDeletion";
import { ActionFailure } from "./HostingActionState";
import * as ui from "./ui";
const tab =
  "-mb-px border-b-2 border-transparent px-1 pb-2.5 text-sm text-muted hover:text-foreground hover:no-underline aria-[current=page]:border-foreground aria-[current=page]:font-medium aria-[current=page]:text-foreground";
const labels = {
  "apps.start": "Start application",
  "apps.stop": "Stop application",
  "apps.restart": "Restart application",
  "apps.rollback": "Rollback application",
  "apps.scale": "Scale application",
} satisfies Record<LifecycleAction, string>;
export function HostingApplication({
  appID,
  view = "overview",
}: {
  appID: string;
  view?: "overview" | "settings" | "logs";
}) {
  const api = useApi();
  const session = useSession();
  const call = useHostingCommand();
  const enabled = api.config.enabledFeatures.includes("hosting");
  const app = useQuery({
    queryKey: ["hosting", "app", appID],
    queryFn: ({ signal }) =>
      call({ action: "apps.get", appID }, appSchema, signal),
    enabled,
  });
  if (!enabled)
    return (
      <Empty title="Hosting is unavailable">
        Hosting is not enabled for this installation.
      </Empty>
    );
  if (app.isPending) return <Loading>Loading application…</Loading>;
  if (app.error)
    return <Failure error={app.error} retry={() => void app.refetch()} />;
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <Link
            className={ui.backLink}
            to="/hosting/projects/$projectID"
            params={{ projectID: app.data.projectID }}
          >
            Project applications
          </Link>
          <h1 className={ui.pageTitle}>{app.data.name}</h1>
          <p className={ui.cx(`${ui.pageLead} flex flex-wrap items-center gap-2`)}>
            <span className={ui.mono}>{app.data.id}</span>
            <span className={ui.cx(`${ui.badge(ui.statusTone(app.data.status))} normal-case`)}>
              Recorded state: {app.data.status}
            </span>
          </p>
        </div>
        <button
          className={ui.button()}
          disabled={app.isFetching}
          onClick={() => void app.refetch()}
        >
          Refresh application
        </button>
      </div>
      {app.data.publicAddress ? <div className="mb-6 flex flex-wrap items-center gap-3 text-sm"><span className={ui.badge(app.data.publicAddress.state==="ready"?"success":app.data.publicAddress.state==="retired"?"neutral":"warning")}>{app.data.publicAddress.state==="ready"?"Public address ready":app.data.publicAddress.state==="retired"?"Public address reserved":"Public address pending"}</span>{app.data.publicAddress.state==="retired"?<span className="break-all">{app.data.publicAddress.hostname}</span>:<a className="break-all" href={app.data.publicAddress.url} target="_blank" rel="noopener noreferrer">{app.data.publicAddress.hostname}</a>}<span className="text-muted">{app.data.publicAddress.state==="retired"?"This application was deleted. Its address remains reserved.":"Your own domain can be added in workspace Domains."}</span></div>:null}
      <nav
        className="mb-8 flex gap-6 overflow-x-auto border-b border-outline"
        aria-label="Application"
      >
        <Link
          className={tab}
          activeOptions={{ exact: true }}
          to="/hosting/apps/$appID"
          params={{ appID }}
          aria-current={view === "overview" ? "page" : undefined}
        >
          Overview
        </Link>
        <Link
          className={tab}
          activeOptions={{ exact: true }}
          to="/hosting/apps/$appID/logs"
          params={{ appID }}
          aria-current={view === "logs" ? "page" : undefined}
        >
          Logs
        </Link>
        <Link
          className={tab}
          activeOptions={{ exact: true }}
          to="/hosting/apps/$appID/settings"
          params={{ appID }}
          aria-current={view === "settings" ? "page" : undefined}
        >
          Settings
        </Link>
      </nav>
      {(session.user.preview && !session.user.acting) ? (
        <p className={ui.cx(`${ui.notice} mb-4`)} role="status">
          Customer preview is read-only. Live log requests and application
          changes are unavailable.
        </p>
      ) : null}
      {app.data.lastError ? (
        <p className="mb-4 rounded-lg border border-danger/40 bg-danger-soft px-4 py-3 text-[13px] text-danger" role="alert">
          Last recorded error: {app.data.lastError}
        </p>
      ) : null}
      {app.data.operationID ? (
        <HostingOperationLink id={app.data.operationID} />
      ) : null}
      {app.data.lifecycle === "draft" ? (
        <Empty title="Awaiting first deployment">
          This application has not been deployed. Deployment source setup is not
          yet available in this Console.
        </Empty>
      ) : (
        <>
          {view === "overview" ? (
            <>
              <ApplicationSummary app={app.data} />
              {app.data.spec.autoscaling ? <HostingScalingStatus key={`${appID}-${String(app.data.revision)}`} app={app.data} /> : null}
              {canManageApp(app.data, (session.user.preview && !session.user.acting) === true) ? (
                <>
                  <ApplicationControls key={appID} app={app.data} />
                  <HostingDeletion key={`delete-${appID}`} app={app.data} />
                </>
              ) : (
                <p className="mt-4 text-muted">
                  Application changes are unavailable with your current access
                  or application state.
                </p>
              )}
            </>
          ) : null}
          {view === "settings" ? (
            canManageApp(app.data, (session.user.preview && !session.user.acting) === true) ? (
              <HostingSettings key={appID} app={app.data} />
            ) : (
              <Empty title="Settings are read-only">
                You cannot change this application with its current state or
                your current access.
              </Empty>
            )
          ) : null}
          {view === "logs" ? (
            <ApplicationLogs key={appID} app={app.data} />
          ) : null}
        </>
      )}
    </section>
  );
}
function ApplicationSummary({ app }: { app: HostingApp }) {
  return (
    <section className={ui.card}>
      <div className={ui.cardBody}>
      <h2 className={ui.sectionTitle}>Configured resources</h2>
      <dl className="grid grid-cols-2 gap-x-6 gap-y-5 md:grid-cols-3">
        {[
          ["Desired revision", String(app.revision)],
          ["Observed revision", String(app.observedRevision)],
          [app.spec.autoscaling ? "Automatic instances" : "Fixed instances", app.spec.autoscaling ? `${String(app.spec.autoscaling.minReplicas)}–${String(app.spec.autoscaling.maxReplicas)}` : String(app.spec.replicas)],
          [
            "CPU per instance",
            formatResource("cpuMillicores", app.spec.cpuMillicores),
          ],
          ["Memory per instance", formatResource("memoryBytes", app.spec.memoryBytes)],
          [
            "Persistent storage",
            formatResource("volumeBytes", app.spec.volumeBytes ?? 0),
          ],
        ].map(([name, value]) => (
          <div key={name}>
            <dt className="text-xs text-muted">{name}</dt>
            <dd className="mt-1 text-sm font-medium">{value}</dd>
          </div>
        ))}
      </dl>
      {app.observedRevision < app.revision ? (
        <p role="status" className={ui.notice}>
          The runtime has not confirmed the desired revision. Review the latest
          operation.
        </p>
      ) : null}
      {app.status === "deleted" ? (
        <p className="text-sm text-muted">
          Application resources are deleted. Any retained storage remains a
          separate resource.
        </p>
      ) : null}
      </div>
    </section>
  );
}
function ApplicationControls({ app }: { app: HostingApp }) {
  const { mutation, retry, uncertain } = useHostingAction();
  useHostingActionCompletion(mutation.data, app.id, mutation.reset);
  const [review, setReview] = useState<{
    action: LifecycleAction;
    app: HostingApp;
  }>();
  useBlocker({
    shouldBlockFn: () =>
      uncertain &&
      !window.confirm(
        "This request's outcome is unknown. Leave this view and review the server state?",
      ),
    enableBeforeUnload: uncertain,
  });
  const pending =
    mutation.isPending ||
    uncertain ||
    Boolean(mutation.data) ||
    app.status === "pending";
  const actions: LifecycleAction[] = [
    app.spec.replicas === 0 ? "apps.start" : "apps.stop",
    "apps.restart",
    "apps.rollback",
    "apps.scale",
  ];
  return (
    <section className={ui.cx(`${ui.card} mt-6`)}>
      <div className={ui.cardBody}>
      <h2 className={ui.sectionTitle}>Application controls</h2>
      <div className="flex flex-wrap gap-2">
        {actions.map((action) => (
          <button
            key={action}
            className={ui.button()}
            disabled={pending || (action === "apps.scale" && Boolean(app.spec.autoscaling))}
            title={action === "apps.scale" && app.spec.autoscaling ? "Switch to fixed instances in Settings to scale manually." : undefined}
            onClick={() => {
              mutation.reset();
              setReview({ action, app });
            }}
          >
            {labels[action]}
          </button>
        ))}
      </div>
      {app.spec.autoscaling ? <p className="mt-3 text-sm text-muted">Automatic scaling controls the instance count. Switch to fixed instances in Settings to choose it manually.</p> : null}
      <ActionFailure
        error={mutation.error}
        uncertain={uncertain}
        retry={retry}
        pending={mutation.isPending}
      />
      {mutation.data ? <HostingReceipt receipt={mutation.data} /> : null}
      </div>
      {review ? (
        <Modal
          title={labels[review.action]}
          close={() => {
            setReview(undefined);
          }}
        >
          <form
            className="grid gap-5 text-sm"
            onSubmit={(event) => {
              event.preventDefault();
              const value = new FormData(event.currentTarget).get("replicas");
              mutation.mutate(
                actionCommand(
                  review.app,
                  review.action,
                  crypto.randomUUID(),
                  Number(value),
                ),
              );
              setReview(undefined);
            }}
          >
            <p>
              This sends a request for revision {review.app.revision}. Traffic
              may be interrupted. The assigned runtime must confirm completion.
            </p>
            {review.action === "apps.scale" ? (
              <label className={ui.field}>
                Replicas
                <input
                  className={ui.input}
                  name="replicas"
                  type="number"
                  min={0}
                  max={20}
                  step={1}
                  defaultValue={review.app.spec.replicas}
                  required
                />
              </label>
            ) : null}
            <button className={ui.cx(`${ui.button("primary")} justify-self-start`)}>
              Confirm {labels[review.action].toLowerCase()}
            </button>
          </form>
        </Modal>
      ) : null}
    </section>
  );
}
function ApplicationLogs({ app }: { app: HostingApp }) {
  const session = useSession();
  const { mutation, retry, uncertain } = useHostingAction();
  const [completed, setCompleted] = useState(false);
  const allowed =
    app.canRequestLogs && !(session.user.preview && !session.user.acting) && app.status !== "deleted";
  return (
    <section className={ui.card}>
      <div className={ui.cardBody}>
      <div className="flex flex-wrap items-start justify-between gap-4">
      <div>
      <h2 className={ui.sectionTitle}>Recorded logs</h2>
      <p className="mt-1 text-sm text-muted">
        This is the last stored, sanitized log snapshot. A refresh queues a new
        read; it is not a live stream.
      </p>
      </div>
      <button
        className={ui.button()}
        disabled={
          !allowed ||
          mutation.isPending ||
          uncertain ||
          (Boolean(mutation.data) && !completed)
        }
        onClick={() => {
          setCompleted(false);
          mutation.mutate(
            commandSchema.parse({
              action: "apps.logs",
              appID: app.id,
              idempotencyKey: crypto.randomUUID(),
            }),
          );
        }}
      >
        {mutation.isPending ? "Requesting…" : "Request fresh logs"}
      </button>
      </div>
      {!allowed ? (
        <p className="text-sm text-muted">
          A live log request requires current access and an observed runtime. No
          current log-read capability is available.
        </p>
      ) : null}
      <ActionFailure
        error={mutation.error}
        uncertain={uncertain}
        retry={retry}
        pending={mutation.isPending}
      />
      {mutation.data ? (
        <HostingLogRead id={mutation.data.id} onTerminal={setCompleted} />
      ) : null}
      </div>
      <div className="border-t border-outline bg-subtle px-6 py-5 max-sm:px-[18px]">
        {app.logs === null ? (
          <p className="text-sm text-muted">No log snapshot has been recorded.</p>
        ) : (
          <pre className="max-h-[60dvh] overflow-auto font-mono text-xs leading-relaxed whitespace-pre-wrap break-all">
            {app.logs || "The recorded snapshot contains no output."}
          </pre>
        )}
      </div>
    </section>
  );
}
