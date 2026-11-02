import { createMailApiKeys } from "@webdock/module-mail/server/product-api-keys";
import { authDatabase } from "../../../db/runtime";
import { withMailDatabase } from "../../../db/database-context";
import { mailDatabase as database } from "./store";
import { assertMailAccess } from "./access";
import { withMailTransaction } from "./store";
export const {
  authorizeMailKey,
  createMailKey,
  createMailKeyInTransaction,
  listMailKeys,
  revokeMailKey,
} = createMailApiKeys({
  authDatabase,
  withMailDatabase,
  database,
  assertMailAccess,
  withMailTransaction,
});
