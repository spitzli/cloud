import {
  moduleForOperation,
  type ProductModule,
} from "@webdock/module-sdk/selection";
import { requireInstalledOperation } from "../../installation";
export type PublicOperationError = {
  kind: string;
  message: string;
  status: number;
};
export interface ProductOperation {
  dispatch(
    operation: string,
    args: unknown[],
    headers: Headers,
    userID: string,
  ): Promise<unknown>;
  publicError(error: unknown): PublicOperationError | undefined;
}
let productLoader: ((module: ProductModule) => Promise<ProductOperation>) | undefined;
export function configureProductOperations(loader: (module: ProductModule) => Promise<ProductOperation>) {
  productLoader = loader;
}
/** Admission precedes importing any optional implementation, even for a denied call. */
export async function loadProductOperation(
  operation: string,
  args: unknown[],
  loader = productLoader,
): Promise<ProductOperation | undefined> {
  const module = moduleForOperation(operation, args);
  if (!module) return;
  requireInstalledOperation(operation, args);
  // Hosting's BYOK commands retain its transport's independent permission checks.
  if (!loader) return;
  return loader(operation === "hosting.command" ? "hosting" : module);
}
