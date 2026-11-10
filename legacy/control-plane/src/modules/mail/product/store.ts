import { createMailStore } from "@spitzli/module-mail/server/product-store";
import { database } from "../../../db/runtime";
export const { mailDatabase, withMailTransaction } = createMailStore(database);
