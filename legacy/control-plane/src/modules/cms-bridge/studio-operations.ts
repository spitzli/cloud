import { dispatchCMS } from "./dispatch";
import { CMSBridgeError } from "./errors";
import type { PublicOperationError } from "../portal/product-operation";
export async function dispatch(operation: string, args: unknown[]) {
  return dispatchCMS(operation, args);
}
export function publicError(error: unknown): PublicOperationError | undefined {
  if (error instanceof CMSBridgeError)
    return {
      kind: "CMSBridgeError",
      message: error.message.slice(0, 300),
      status: 400,
    };
}
