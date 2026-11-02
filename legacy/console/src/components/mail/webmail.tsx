"use client";
import { Webmail as Mailbox } from "@webdock/module-mail/ui/webmail";
import { launchWebmail } from "@/lib/webmail-actions";
export function Webmail(props: {
  customerID: string;
  mailboxID: string;
  address: string;
}) {
  return <Mailbox {...props} launchAction={launchWebmail} />;
}
