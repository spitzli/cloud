import { createMailBackfill } from "@webdock/module-mail/server/product-backfill";
import { withMailTransaction } from "./store";
export { type ByokBackfillMapping } from "@webdock/module-mail/server/product-backfill";
export const { backfillMailConnections } = createMailBackfill({
  withMailTransaction,
});
