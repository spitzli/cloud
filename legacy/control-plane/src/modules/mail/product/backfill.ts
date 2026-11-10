import { createMailBackfill } from "@spitzli/module-mail/server/product-backfill";
import { withMailTransaction } from "./store";
export { type ByokBackfillMapping } from "@spitzli/module-mail/server/product-backfill";
export const { backfillMailConnections } = createMailBackfill({
  withMailTransaction,
});
