import { createMailEvents } from "@spitzli/module-mail/server/product-events";
import { withMailTransaction } from "./store";
export const { recordMailEvent } = createMailEvents({ withMailTransaction });
