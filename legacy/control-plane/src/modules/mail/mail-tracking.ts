import { TrackingClient } from "./turbo-tracking";
import { createTenantTracking } from "@webdock/module-mail/server/tenant-tracking";
import { resolveSessionReference } from "../auth/principal";
import { withAuthenticatedDatabase } from "../../db/database-context";
import { productTrackingContext } from "./product/tracking-context";
import { database } from "../../db/runtime";
import { getTenant } from "../access/tenants";
import {
  getPlatformSettings,
  getMailConnectionStatus,
  mailProvider,
} from "../platform/platform";
export {
  TrackingError,
  trackingSchemaSQL,
} from "@webdock/module-mail/server/tenant-tracking";
export const { trackingHostname, getTenantTracking, manageTenantTracking } =
  createTenantTracking({
    resolveSessionReference,
    withAuthenticatedDatabase,
    productTrackingContext,
    database,
    getTenant,
    getPlatformSettings,
    getMailConnectionStatus,
    mailProvider,
    trackingClient: (auth, fetcher) => new TrackingClient(auth, fetcher),
  });
