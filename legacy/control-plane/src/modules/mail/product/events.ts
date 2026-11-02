import { createMailEvents } from "@webdock/module-mail/server/product-events";
import { withMailTransaction } from "./store";
export const { recordMailEvent } = createMailEvents({ withMailTransaction });
