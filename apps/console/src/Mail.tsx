import { useEffect, useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useApi, useSession } from "./context";
import { ApiError } from "./api";
import { Empty, Failure, Loading, Modal } from "./common";
import { tenantsSchema } from "./hosting-contracts";
import { useMailCommand } from "./mail-client";
import {
  directorySchema,
  lifecycleResolved,
  serviceSchema,
  type MailAction,
  type MailService,
} from "./mail-contracts";
import { MailConfiguration } from "./MailConfiguration";
import { MailDirectoryView } from "./MailDirectory";
import * as ui from "./ui";
export function Mail({ customerID }: { customerID?: string | undefined }) {
  const api = useApi();
  const navigate = useNavigate();
  const enabled = api.config.enabledFeatures.includes("mail");
  const tenants = useQuery({
    queryKey: ["tenants"],
    queryFn: ({ signal }) =>
      api.get("/api/console/tenants", tenantsSchema, signal),
    enabled,
    staleTime: 0,
    refetchOnMount: "always",
  });
  if (!enabled)
    return (
      <Empty title="Mail is unavailable">
        Mail is not enabled for this installation.
      </Empty>
    );
  if (tenants.error)
    return (
      <Failure error={tenants.error} retry={() => void tenants.refetch()} />
    );
  if (tenants.isPending || !tenants.isFetchedAfterMount)
    return <Loading>Loading workspaces…</Loading>;
  const selected = tenants.data.some((tenant) => tenant.id === customerID);
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>Mail</h1>
          <p className={ui.pageLead}>
            Incoming mail is activated explicitly for each workspace.
          </p>
        </div>
      </div>
      <label className={ui.cx(`${ui.field} mb-6 max-w-[420px]`)}>
        Workspace
        <select
          className={ui.input}
          aria-label="Workspace"
          value={selected ? customerID : ""}
          onChange={(event) => {
            void navigate({
              to: "/mail",
              search: { customer: event.target.value || undefined },
            });
          }}
        >
          <option value="">Choose a workspace</option>
          {tenants.data.map((tenant) => (
            <option key={tenant.id} value={tenant.id}>
              {tenant.name}
            </option>
          ))}
        </select>
      </label>
      {selected && customerID ? (
        <MailCustomer key={customerID} customerID={customerID} />
      ) : (
        <Empty title="Choose a workspace">
          View its incoming mail service and mailbox directory.
        </Empty>
      )}
    </section>
  );
}
type RequestState = {
  revision: string;
  kind: "pending" | "accepted" | "uncertain" | "rejected" | "denied";
} | null;
export function MailCustomer({ customerID }: { customerID: string }) {
  const call = useMailCommand();
  const client = useQueryClient();
  const session = useSession();
  const [confirm, setConfirm] = useState<MailAction | null>(null);
  const ledgerKey = ["mail-request", session.session.id, customerID];
  const ledger = useQuery<RequestState>({
    queryKey: ledgerKey,
    queryFn: () => null,
    initialData: null,
    enabled: false,
    gcTime: Infinity,
  });
  const service = useQuery({
    queryKey: ["mail", session.session.id, customerID, "service"],
    queryFn: async ({ signal }) => {
      const result = await call(
        { action: "getTenantMailService", customerID },
        serviceSchema,
        signal,
      );
      if (result.data.service.customerID !== customerID)
        throw new Error("Mail workspace does not match.");
      return result.data;
    },
    staleTime: 0,
    refetchOnMount: "always",
    retry: false,
    refetchInterval: (query) =>
      query.state.status !== "error" &&
      ["pending", "provisioning"].includes(
        query.state.data?.service.state ?? "",
      )
        ? 2000
        : false,
  });
  const directory = useQuery({
    queryKey: ["mail", session.session.id, customerID, "directory"],
    queryFn: async ({ signal }) => {
      const result = await call(
        { action: "getTenantMailboxes", customerID },
        directorySchema,
        signal,
      );
      if (result.data.customerID !== customerID)
        throw new Error("Mailbox workspace does not match.");
      return result.data;
    },
    enabled:
      service.isSuccess &&
      service.isFetchedAfterMount &&
      service.data.mailboxesAvailable,
    staleTime: 0,
    refetchOnMount: "always",
    retry: false,
    refetchInterval: (query) =>
      query.state.status !== "error" &&
      ["pending", "running"].includes(
        query.state.data?.configuration?.state ?? "",
      )
        ? 2000
        : false,
  });
  useEffect(() => {
    if (
      service.isSuccess &&
      service.isFetchedAfterMount &&
      service.data.mailboxesAvailable
    )
      void client.invalidateQueries({
        queryKey: ["mail", session.session.id, customerID, "directory"],
      });
  }, [
    client,
    customerID,
    session.session.id,
    service.data,
    service.isSuccess,
    service.isFetchedAfterMount,
  ]);
  const refresh = async () => {
    const result = await service.refetch();
    const boxes = result.data?.mailboxesAvailable
      ? await directory.refetch()
      : null;
    if (
      !result.isError &&
      !boxes?.isError &&
      (ledger.data?.kind === "denied" || ledger.data?.kind === "rejected")
    )
      client.setQueryData<RequestState>(ledgerKey, null);
  };
  const mutation = useMutation({
    mutationFn: (input: { action: MailAction; revision: string }) =>
      call(
        { action: "manageTenantMailService", customerID, input },
        serviceSchema,
      ),
    retry: false,
    gcTime: 0,
    onMutate: (input) => {
      client.setQueryData<RequestState>(ledgerKey, () => ({
        revision: input.revision,
        kind: "pending",
      }));
    },
    onSuccess: (_result, input) => {
      client.setQueryData<RequestState>(ledgerKey, () => ({
        revision: input.revision,
        kind: "accepted",
      }));
      void refresh();
    },
    onError: (error, input) => {
      const kind =
        error instanceof ApiError && error.status < 500
          ? [401, 403].includes(error.status)
            ? "denied"
            : "rejected"
          : "uncertain";
      client.setQueryData<RequestState>(ledgerKey, () => ({
        revision: input.revision,
        kind,
      }));
      void client.invalidateQueries({
        queryKey: ["mail", session.session.id, customerID],
      });
    },
  });
  const settled =
    service.isSuccess &&
    service.isFetchedAfterMount &&
    ledger.data &&
    ["accepted", "uncertain"].includes(ledger.data.kind) &&
    lifecycleResolved(service.data, ledger.data.revision);
  useEffect(() => {
    if (settled)
      client.setQueryData<RequestState>(
        ["mail-request", session.session.id, customerID],
        null,
      );
  }, [settled, client, customerID, session.session.id]);
  if (ledger.data?.kind === "denied")
    return (
      <Failure
        error={
          new Error(
            "Mail access changed. Refresh to verify your current access.",
          )
        }
        retry={() => void refresh()}
      />
    );
  if (service.error || directory.error)
    return (
      <Failure
        error={
          service.error ?? directory.error ?? new Error("Mail is unavailable.")
        }
        retry={() => void refresh()}
      />
    );
  if (service.isPending || !service.isFetchedAfterMount)
    return <Loading>Loading Mail…</Loading>;
  if (
    service.data.mailboxesAvailable &&
    (directory.isPending || !directory.isFetchedAfterMount)
  )
    return <Loading>Loading mailbox directory…</Loading>;
  const blocked =
    Boolean((session.user.preview && !session.user.acting)) ||
    ledger.data !== null ||
    service.isFetching ||
    directory.isFetching;
  return (
    <div className="grid gap-6">
      <section className={ui.card}>
        <div className={ui.cardBody}>
          <div className="flex flex-wrap items-start justify-between gap-4">
            <div>
              <h2 className={ui.sectionTitle}>Incoming mail</h2>
              <p className="mt-1 text-sm text-muted">
                Outgoing-only mail does not require an incoming mail instance.
              </p>
            </div>
            <button
              className={ui.button()}
              disabled={service.isFetching || directory.isFetching}
              onClick={() => void refresh()}
            >
              Refresh Mail
            </button>
          </div>
          {(session.user.preview && !session.user.acting) ? (
            <p className={ui.notice}>Customer preview is read-only.</p>
          ) : null}
          <ServiceSummary value={service.data} />
          {ledger.data ? <RequestNotice kind={ledger.data.kind} /> : null}
        </div>
        <div className={ui.cx(`${ui.cardFooter} justify-end`)}>
          {actions.map(({ action, label }) => (
            <button
              key={action}
              className={ui.button()}
              disabled={blocked || !permitted(service.data, action)}
              onClick={() => {
                setConfirm(action);
              }}
            >
              {label}
            </button>
          ))}
        </div>
      </section>
      {directory.data && service.data.mailboxesAvailable ? (
        <>
          <MailDirectoryView value={directory.data} />
          <MailConfiguration
            value={directory.data}
            readAt={directory.dataUpdatedAt}
            refresh={refresh}
            provisioningBlocked={
              Boolean(ledger.data) || service.isFetching || directory.isFetching
            }
          />
        </>
      ) : (
        <Empty title="Mailbox directory is unavailable">
          Mailbox access has not been enabled for this installation.
        </Empty>
      )}
      {confirm ? (
        <Modal
          title={`${confirm.charAt(0).toUpperCase()}${confirm.slice(1)} incoming mail`}
          close={() => {
            setConfirm(null);
          }}
        >
          <p className="mb-5 text-sm">
            {confirm === "suspend"
              ? "Incoming mail will stop. Mailboxes, messages, credentials and storage are retained. Suspension does not delete customer data."
              : "This requests a change to the incoming mail service. The recorded state changes only after the agent reports its result."}
          </p>
          <button
            className={ui.button("primary")}
            disabled={blocked || !permitted(service.data, confirm)}
            onClick={() => {
              mutation.mutate({
                action: confirm,
                revision: service.data.service.revision,
              });
              setConfirm(null);
            }}
          >
            Confirm {confirm}
          </button>
        </Modal>
      ) : null}
    </div>
  );
}
const actions = [
  { action: "activate", label: "Activate incoming mail" },
  { action: "suspend", label: "Suspend incoming mail" },
  {
    action: "reconcile",
    label: "Reconcile incoming mail",
  },
] as const;
function permitted(service: MailService, action: MailAction): boolean {
  switch (action) {
    case "activate":
      return service.canActivate && !service.service.enabled;
    case "suspend":
      return service.canSuspend;
    case "reconcile":
      return service.canReconcile;
  }
}
function ServiceSummary({ value }: { value: MailService }) {
  return (
    <section className="grid gap-4">
      <dl className="grid gap-4 sm:grid-cols-3">
        <div>
          <dt className="text-xs text-muted">Recorded state</dt>
          <dd className="mt-1 text-sm">{value.service.state.replaceAll("_", " ")}</dd>
        </div>
        <div>
          <dt className="text-xs text-muted">Requested direction</dt>
          <dd className="mt-1 text-sm">{value.service.enabled ? "Enabled" : "Suspended / disabled"}</dd>
        </div>
        <div>
          <dt className="text-xs text-muted">Desired revision</dt>
          <dd className={ui.cx(`mt-1 ${ui.mono}`)}>{value.service.revision}</dd>
        </div>
      </dl>
      {value.pendingReview ? (
        <p role="alert" className="text-sm text-danger">
          An earlier operation needs review. A requested direction is not proof
          of readiness.
        </p>
      ) : null}
    </section>
  );
}
function RequestNotice({ kind }: { kind: NonNullable<RequestState>["kind"] }) {
  const messages = {
    pending: [
      "Request in progress",
      "Waiting for the server to record this request.",
    ],
    accepted: [
      "Request recorded",
      "The change is queued. Refresh to check its observed state.",
    ],
    uncertain: [
      "Request outcome is unknown",
      "The response was lost or unavailable. Do not submit another change. Refresh until a newer settled revision is confirmed.",
    ],
    rejected: [
      "Request was not accepted",
      "Refresh Mail and review the current revision and permissions before submitting another change.",
    ],
    denied: ["Access changed", "Refresh to verify access."],
  };
  const message = messages[kind];
  return (
    <section className={ui.notice} role="status">
      <h3 className="text-sm font-semibold">{message[0]}</h3>
      <p className="mt-1 text-muted">{message[1]}</p>
    </section>
  );
}
