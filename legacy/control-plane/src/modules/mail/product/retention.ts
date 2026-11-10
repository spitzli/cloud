import { createMailRetention } from "@spitzli/module-mail/server/product-retention";
import { withMailTransaction } from "./store";
export const { pruneMailData } = createMailRetention({ withMailTransaction });
