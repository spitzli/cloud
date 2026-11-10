import { createMailConnections } from "@spitzli/module-mail/server/product-connections";
import { mailDatabase as database, withMailTransaction } from "./store";
import { assertMailAccess } from "./access";
export const { getMailConnections, pauseMailConnection } =
  createMailConnections({ database, withMailTransaction, assertMailAccess });
