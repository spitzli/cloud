import { useEffect } from "react";
import { Link } from "@tanstack/react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useHostingCommand } from "./hosting-client";
import {
  operationSchema,
  operationPolling,
  logReadSchema,
  type Receipt,
  type HostingApp,
} from "./hosting-contracts";
import { useApi } from "./context";
import { Failure, Loading } from "./common";
import * as ui from "./ui";
function useHostingOperation(id: string) {
  const call = useHostingCommand();
  const query = useQuery({
    queryKey: ["hosting-operation", id],
    queryFn: ({ signal }) =>
      call(
        { action: "operations.get", operationID: id },
        operationSchema,
        signal,
      ),
    refetchInterval: (value) =>
      operationPolling(
        value.state.data?.status,
        value.state.status === "error",
      ),
  });
  useRefreshHosting(query.data?.status, id);
  return query;
}
function useRefreshHosting(status: string | undefined, id: string) {
  const client = useQueryClient();
  useEffect(() => {
    if (status && operationPolling(status, false) === false)
      void client.invalidateQueries({ queryKey: ["hosting"] });
  }, [client, id, status]);
}
export function HostingOperation({ id }: { id: string }) {
  const query = useHostingOperation(id);
  if (query.isPending) return <Loading>Loading operation…</Loading>;
  if (query.error)
    return <Failure error={query.error} retry={() => void query.refetch()} />;
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>Hosting operation</h1>
          <p className={ui.pageLead}>
            {query.data.action} · <span className={ui.mono}>{query.data.id}</span>
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
      <div className={ui.card}>
      <dl className={ui.cx(`${ui.cardBody} sm:grid-cols-2`)}>
        <div>
          <dt className="text-xs text-muted">Status</dt>
          <dd className="mt-1 text-sm font-medium" role="status">{query.data.status}</dd>
        </div>
        <div>
          <dt className="text-xs text-muted">Requested</dt>
          <dd className="mt-1 text-sm">
            {query.data.createdAt
              ? new Date(query.data.createdAt).toLocaleString()
              : "Not recorded"}
          </dd>
        </div>
      </dl>
      <div className={ui.cardFooter}>
      <span>{operationDescription(query.data.status)}</span>
      {query.data.projectID ? (
        <Link
          className={ui.button()}
          to="/hosting/projects/$projectID"
          params={{ projectID: query.data.projectID }}
        >
          Back to applications
        </Link>
      ) : null}
      </div>
      </div>
    </section>
  );
}
export function operationDescription(status: string): string {
  switch (status) {
    case "queued":
      return "The request is queued. The runtime has not completed it. If it remains queued, check the assigned agent.";
    case "running":
      return "The agent is processing this request. Completion has not been confirmed.";
    case "succeeded":
      return "Completed.";
    case "failed":
      return "The operation failed. Review the application’s last error before submitting another request.";
    case "needs-reconciliation":
      return "The operation needs operator reconciliation. Its outcome must be reviewed before another change.";
    default:
      return "The operation state is unavailable.";
  }
}
export function HostingReceipt({ receipt }: { receipt: Receipt }) {
  return (
    <div role="status" className={ui.cx(`${ui.notice} grid gap-2`)}>
      <div className="grid gap-2">
        <h2 className="text-sm font-semibold">Request recorded</h2>
        <p className="text-muted">{operationDescription(receipt.status)}</p>
        <Link
          className={ui.cx(`${ui.button()} justify-self-start`)}
          to="/hosting/operations/$operationID"
          params={{ operationID: receipt.id }}
        >
          View operation
        </Link>
      </div>
    </div>
  );
}

export function HostingOperationLink({ id }: { id: string }) {
  const query = useHostingOperation(id);
  if (query.error)
    return <Failure error={query.error} retry={() => void query.refetch()} />;
  return (
    <div className={ui.cx(`${ui.notice} mb-4`)} role="status">
      <div className="grid gap-1">
        <strong className="font-semibold">Latest operation: {query.data?.status ?? "Loading…"}</strong>
        <p className="text-muted">{operationDescription(query.data?.status ?? "")}</p>
        <Link
          className="justify-self-start font-medium underline underline-offset-3"
          aria-label="View latest operation"
          to="/hosting/operations/$operationID"
          params={{ operationID: id }}
        >
          View operation
        </Link>
      </div>
    </div>
  );
}
export function HostingLogRead({
  id,
  onTerminal,
}: {
  id: string;
  onTerminal: (complete: boolean) => void;
}) {
  const api = useApi();
  const query = useQuery({
    queryKey: ["hosting-read", id],
    queryFn: ({ signal }) =>
      api.get(
        `/api/console/hosting/reads/${encodeURIComponent(id)}`,
        logReadSchema,
        signal,
      ),
    refetchInterval: (value) =>
      operationPolling(
        value.state.data?.status,
        value.state.status === "error",
      ),
  });
  useRefreshHosting(query.data?.status, id);
  const status = query.data?.status;
  useEffect(() => {
    if (status && !operationPolling(status, false)) onTerminal(true);
  }, [id, onTerminal, status]);
  if (query.isPending) return <Loading>Checking the log request…</Loading>;
  if (query.error)
    return <Failure error={query.error} retry={() => void query.refetch()} />;
  return (
    <section className={ui.cx(`${ui.notice} grid gap-2`)} aria-label="Requested log snapshot">
      <h3 className="text-sm font-semibold">Log request: {query.data.status}</h3>
      <p role="status" className="text-muted">{operationDescription(query.data.status)}</p>
      {query.data.error ? (
        <p role="alert" className="text-danger">
          {query.data.error}
        </p>
      ) : null}
      {query.data.result ? (
        <>
          <pre className="mt-2 max-h-[60dvh] overflow-auto rounded-md border border-outline bg-surface p-4 font-mono text-xs leading-relaxed whitespace-pre-wrap break-all">
            {query.data.result.logs || "No log output."}
          </pre>
          {query.data.result.truncated ? (
            <p className="text-muted">
              This snapshot was truncated to the permitted size.
            </p>
          ) : null}
        </>
      ) : null}
    </section>
  );
}

export function useHostingActionCompletion(
  receipt: Receipt | undefined,
  appID: string,
  onComplete: () => void,
) {
  const client = useQueryClient();
  const call = useHostingCommand();
  const operation = useQuery({
    queryKey: ["hosting-operation", receipt?.id],
    queryFn: ({ signal }) =>
      call(
        { action: "operations.get", operationID: receipt?.id },
        operationSchema,
        signal,
      ),
    enabled: receipt !== undefined,
    staleTime: 0,
    refetchInterval: (value) =>
      operationPolling(
        value.state.data?.status,
        value.state.status === "error",
      ),
  });
  const status = operation.data?.status;
  useEffect(() => {
    if (
      !receipt ||
      operation.error ||
      operation.isFetching ||
      !finished(status)
    )
      return;
    let current = true;
    const refresh = async () => {
      const key = ["hosting", "app", appID];
      await client.cancelQueries({ queryKey: key, exact: true });
      await client.refetchQueries(
        { queryKey: key, exact: true, type: "active" },
        { cancelRefetch: true, throwOnError: true },
      );
      const app = client.getQueryState<HostingApp>(key);
      const confirmed = client.getQueryState<
        ReturnType<typeof operationSchema.parse>
      >(["hosting-operation", receipt.id]);
      if (
        current &&
        confirmed?.status === "success" &&
        confirmed.fetchStatus === "idle" &&
        finished(confirmed.data?.status) &&
        refreshedApplication(app, appID, receipt.revision)
      )
        onComplete();
    };
    void refresh().catch(() => {
      /* Keep the receipt locked; the application query exposes refresh errors. */
    });
    return () => {
      current = false;
    };
  }, [
    appID,
    client,
    onComplete,
    operation.error,
    operation.isFetching,
    receipt,
    status,
  ]);
}
function finished(status: string | undefined) {
  return status === "succeeded" || status === "failed";
}
function refreshedApplication(
  state:
    | { status: string; fetchStatus: string; data: HostingApp | undefined }
    | undefined,
  appID: string,
  revision: number | undefined,
) {
  if (
    state?.status !== "success" ||
    state.fetchStatus !== "idle" ||
    state.data?.id !== appID
  )
    return false;
  return state.data.revision >= (revision ?? 0);
}
