import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useApi, useSession } from "./context";
import { Empty, Failure, Loading } from "./common";
import { useMailCommand } from "./mail-client";
import { directorySchema, serviceSchema } from "./mail-contracts";
import { launchSchema } from "./webmail-contracts";
import { openWebmail, type WebmailConnection } from "./webmail-client";
import { WebmailWorkspace } from "./WebmailWorkspace";

export function Webmail({ customerID, mailboxID }: { customerID: string | undefined; mailboxID: string }) {
  const api = useApi();
  const session = useSession();
  const call = useMailCommand();
  const origin = api.config.mailGatewayOrigin;
  const enabled = Boolean(customerID && origin && api.config.enabledFeatures.includes("mail") && !(session.user.preview && !session.user.acting));
  const directory = useQuery({
    queryKey: ["mail", session.session.id, customerID, "directory"],
    queryFn: async ({ signal }) => {
      if (!customerID) throw new Error("Choose a workspace.");
      const value = (await call({ action: "getTenantMailboxes", customerID }, directorySchema, signal)).data;
      if (value.customerID !== customerID) throw new Error("Mailbox workspace does not match.");
      return value;
    },
    enabled, staleTime: 0, refetchOnMount: "always", refetchInterval: 30000, retry: false,
  });
  const service = useQuery({
    queryKey: ["mail", session.session.id, customerID, "service"],
    queryFn: async ({ signal }) => {
      if (!customerID) throw new Error("Choose a workspace.");
      const value = (await call({ action: "getTenantMailService", customerID }, serviceSchema, signal)).data;
      if (value.service.customerID !== customerID) throw new Error("Mail workspace does not match.");
      return value;
    },
    enabled, staleTime: 0, refetchOnMount: "always", refetchInterval: 30000, retry: false,
  });
  if (!enabled || !customerID || !origin) return <Empty title="Webmail is unavailable">Select an authorized mailbox outside customer preview. Webmail also requires a configured gateway.</Empty>;
  if (directory.error || service.error) return <Failure error={directory.error ?? service.error ?? new Error("Mail access is unavailable.")} />;
  if (!directory.isSuccess || !directory.isFetchedAfterMount || !service.isSuccess || !service.isFetchedAfterMount) return <Loading>Checking mailbox access…</Loading>;
  const mailbox = directory.data.mailboxes.find((item) => item.id === mailboxID && item.canOpen);
  if (!mailbox) return <Empty title="Mailbox access is unavailable">A current explicit mailbox grant is required. Operator status alone does not grant content access.</Empty>;
  return <MailboxSession key={`${session.session.id}:${customerID}:${mailboxID}`} customerID={customerID} mailboxID={mailboxID} name={mailbox.name} address={mailbox.address} origin={origin} sendingAvailable={service.data.sendingAvailable} />;
}
function MailboxSession(props: { customerID: string; mailboxID: string; name: string; address: string; origin: string; sendingAvailable: boolean }) {
  const call = useMailCommand();
  const [connection, setConnection] = useState<WebmailConnection | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const { customerID, mailboxID, origin } = props;
  useEffect(() => {
    const controller = new AbortController();
    let current: WebmailConnection | undefined;
    void openWebmail(
      (signal) => call({ action: "launchTenantWebmail", customerID, mailboxID }, launchSchema, signal),
      origin, customerID, mailboxID, controller.signal,
    ).then((value) => {
      current = value;
      if (controller.signal.aborted) { value.close(); return; }
      setConnection(value);
    }).catch(() => {
      if (!controller.signal.aborted) setError(new Error("Webmail could not connect. Verify access and gateway availability before reconnecting."));
    });
    return () => { controller.abort(); current?.close(); };
  }, [call, customerID, mailboxID, origin]);
  if (error) return <Failure error={error} />;
  if (!connection) return <Loading>Opening mailbox…</Loading>;
  return <ConnectedWebmail {...props} connection={connection} />;
}
export function ConnectedWebmail({ connection, customerID, mailboxID, name, address, sendingAvailable }: {
  connection: WebmailConnection; customerID: string; mailboxID: string; name: string; address: string; sendingAvailable: boolean;
}) {
  const client = useQueryClient();
  const session = useSession();
  const [failure, setFailure] = useState<Error | null>(null);
  useEffect(() => {
    const retire = () => {
      connection.close();
      client.removeQueries({ queryKey: ["webmail", session.session.id, customerID, mailboxID] });
      setFailure(new Error("Mail access expired or was revoked. Return to Mail and verify your access."));
    };
    const unsubscribe = connection.onDenied(retire);
    const timer = setTimeout(retire, Math.max(0, connection.expiresAt - Date.now()));
    // MailboxSession owns the connection and closes it; closing here would break the StrictMode remount.
    return () => { clearTimeout(timer); unsubscribe(); };
  }, [connection, client, customerID, mailboxID, session.session.id]);
  if (failure || connection.isClosed()) return <Failure error={failure ?? new Error("Mail access expired or was revoked.")} />;
  return <WebmailWorkspace connection={connection} customerID={customerID} mailboxID={mailboxID} name={name} address={address} sendingAvailable={sendingAvailable} />;
}
