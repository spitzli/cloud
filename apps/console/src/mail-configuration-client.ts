import { useEffect, useRef } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useSession } from "./context";
import { ApiError } from "./api";
import { useMailCommand } from "./mail-client";
import {
  mailboxChangeSchema,
  mailboxCommandSchema,
  type MailboxCommand,
  type MailDirectory,
} from "./mail-contracts";
export function configurationRequest(
  value: MailDirectory,
  input: MailboxCommand,
  readAt: number,
) {
  const domainID = input.action === "mailbox.create" ? input.domainID : null;
  const domainName =
    input.action === "domain.attach"
      ? value.verifiedDomains.find(
          (domain) => domain.id === input.senderDomainID,
        )?.domain
      : value.domains.find((domain) => domain.id === domainID)?.name;
  return {
    input,
    readAt,
    customerID: value.customerID,
    domainName: domainName ?? null,
    beforeRevision: value.configuration?.revision ?? "0",
  };
}
export type ConfigurationRequest = ReturnType<typeof configurationRequest>;
export function configurationObserved(
  request: ConfigurationRequest,
  value: MailDirectory,
  readAt: number,
): boolean {
  if (readAt <= request.readAt || value.customerID !== request.customerID)
    return false;
  const input = request.input;
  switch (input.action) {
    case "domain.attach":
      return value.domains.some((domain) => domain.name === request.domainName);
    case "mailbox.create":
      return value.mailboxes.some(
        (mailbox) =>
          mailbox.address ===
            `${input.localPart}@${request.domainName ?? ""}` &&
          mailbox.name === input.name &&
          mailbox.quotaBytes === input.quotaBytes,
      );
    case "grant":
      return value.grants.some(
        (grant) =>
          grant.mailboxID === input.mailboxID &&
          grant.subject === input.subject &&
          grant.enabled,
      );
    case "revoke":
      return value.grants.some(
        (grant) =>
          grant.mailboxID === input.mailboxID &&
          grant.subject === input.subject &&
          !grant.enabled,
      );
    case "configuration.retry":
      return (
        BigInt(value.configuration?.revision ?? "0") >
        BigInt(request.beforeRevision)
      );
  }
}
type RequestKind = "pending" | "accepted" | "uncertain" | "rejected" | "denied";
type Ledger = { request: ConfigurationRequest; kind: RequestKind } | null;
export function useConfigurationAction(
  value: MailDirectory,
  readAt: number,
  lane: "configure" | "revoke",
  refresh: () => Promise<void>,
) {
  const inFlight = useRef(false);
  const call = useMailCommand();
  const client = useQueryClient();
  const session = useSession();
  const key = [
    "mail-configuration-request",
    session.session.id,
    value.customerID,
    lane,
  ];
  const queries = ["mail", session.session.id, value.customerID];
  const ledger = useQuery<Ledger>({
    queryKey: key,
    queryFn: () => null,
    initialData: null,
    enabled: false,
    gcTime: Infinity,
  });
  const mutation = useMutation({
    retry: false,
    gcTime: 0,
    mutationFn: (request: ConfigurationRequest) =>
      call(
        {
          action: "manageTenantMailboxes",
          customerID: value.customerID,
          input: request.input,
        },
        mailboxChangeSchema,
      ),
    onSettled: () => {
      inFlight.current = false;
    },
    onMutate: (request) => {
      client.setQueryData<Ledger>(key, () => ({ request, kind: "pending" }));
    },
    onSuccess: (_result, request) => {
      client.setQueryData<Ledger>(key, () => ({ request, kind: "accepted" }));
      void client.invalidateQueries({ queryKey: queries });
    },
    onError: (error, request) => {
      const denied =
        error instanceof ApiError && [401, 403].includes(error.status);
      const kind =
        error instanceof ApiError && error.status < 500
          ? denied
            ? "denied"
            : "rejected"
          : "uncertain";
      client.setQueryData<Ledger>(key, () => ({ request, kind }));
      if (denied) {
        void client.resetQueries({ queryKey: queries });
        return;
      }
      void client.invalidateQueries({ queryKey: queries });
    },
  });
  const observed =
    ledger.data &&
    ["accepted", "uncertain"].includes(ledger.data.kind) &&
    configurationObserved(ledger.data.request, value, readAt);
  useEffect(() => {
    if (observed)
      client.setQueryData<Ledger>(
        [
          "mail-configuration-request",
          session.session.id,
          value.customerID,
          lane,
        ],
        null,
      );
  }, [observed, client, session.session.id, value.customerID, lane]);
  const submit = (input: MailboxCommand) => {
    if (
      inFlight.current ||
      ledger.data ||
      (session.user.preview && !session.user.acting) ||
      !value.canManage
    )
      return;
    if (input.action !== "revoke" && !value.canConfigure) return;
    const request = configurationRequest(
      value,
      mailboxCommandSchema.parse(input),
      readAt,
    );
    inFlight.current = true;
    mutation.mutate(request);
  };
  const review = async () => {
    await refresh();
    const current = client.getQueryState([...queries, "directory"]);
    if (current?.status !== "success" || current.dataUpdatedAt <= readAt)
      return;
    if (ledger.data?.kind === "denied" || ledger.data?.kind === "rejected")
      client.setQueryData<Ledger>(key, null);
  };
  return {
    blocked: ledger.data !== null,
    state: ledger.data?.kind ?? null,
    submit,
    review,
  };
}
