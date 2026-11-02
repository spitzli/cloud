import { createMailHttp } from "@webdock/module-mail/server/product-http";
import { withMailDatabase } from "../../../db/database-context";
import { authorizeMailKey } from "./api-keys";
import { consumeMailRate } from "./rate-limit";
import { mailDatabase as database } from "./store";
export { type MailRouteContext } from "@webdock/module-mail/server/product-http";
export const { readBoundedText, readBoundedJson, mailEndpoint } =
  createMailHttp({
    withMailDatabase,
    authorizeMailKey,
    consumeMailRate,
    database,
  });
