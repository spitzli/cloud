import { readCachedView } from "../../cache";
import { createHostingViewCache } from "@spitzli/module-hosting/server/view-cache";
export const { readHostingView } = createHostingViewCache(readCachedView);
