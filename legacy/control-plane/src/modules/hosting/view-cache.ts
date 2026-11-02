import { readCachedView } from "../../cache";
import { createHostingViewCache } from "@webdock/module-hosting/server/view-cache";
export const { readHostingView } = createHostingViewCache(readCachedView);
