import { Pool } from "pg";
import { createMailImport } from "@spitzli/module-mail/server/product-import";
import { encryptMailSecret } from "../../platform/platform";
import { withMailTransaction } from "./store";
import { persistMailMessage } from "./sending";
import { canonicalHash } from "./idempotency";
import { recordMailEvent } from "./events";
export { type RelaykitMapping } from "@spitzli/module-mail/server/product-import";
export const { importRelaykit } = createMailImport({
  encryptMailSecret,
  createImportPool: (config) => new Pool(config),
  withMailTransaction,
  persistMailMessage,
  canonicalHash,
  recordMailEvent,
});
