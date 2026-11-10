import { demoAccess } from "./demo-bridges";
import { createCMSStudioShop } from "@spitzli/module-cms/server/studio-shop";
export {
  type ShopKind,
  type ShopOption,
  type ShopProduct,
  type ShopContent,
  type ShopOrder,
  type ShopAddress,
  type ShopMedia,
  type ShopCustomer,
  type ShopResult,
} from "@spitzli/module-cms/server/studio-shop";
export const { shopAccess, shopRequest } = createCMSStudioShop({
  demoAccess,
  fetch: (input, init) => globalThis.fetch(input, init),
});
