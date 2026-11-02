import { createMailTrackingContext } from "@webdock/module-mail/server/product-tracking-context";
import { withAuthenticatedDatabase } from "../../../db/database-context";
import { mailDatabase as database } from "./store";
import { authorizeMailSession } from "./access";
import { assertMailAccess } from "./access";
import {
  mailManagementHeaders,
  resolveSendingCredential,
} from "./provider-credentials";
import { TrackingClient } from "../turbo-tracking";
export const { productTrackingContext } = createMailTrackingContext({
  withAuthenticatedDatabase,
  database,
  authorizeMailSession,
  assertMailAccess,
  mailManagementHeaders,
  resolveSendingCredential,
  trackingClient: (auth, fetcher) => new TrackingClient(auth, fetcher),
});
