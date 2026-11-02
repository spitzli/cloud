import { TurboDomainsClient } from "../mail/turbo-domains";
import type { HostingPorts } from "@webdock/module-hosting/server/ports";
export const domainProvider: HostingPorts["domainProvider"] = (auth, fetcher) =>
  new TurboDomainsClient(auth, fetcher);
