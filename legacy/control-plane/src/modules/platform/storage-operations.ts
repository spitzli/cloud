import {
  StorageUsageError,
  getTenantStorage,
  manageTenantStorage,
} from "./storage-usage";
import type { PublicOperationError } from "../portal/product-operation";
export async function dispatch(
  operation: string,
  args: unknown[],
  headers: Headers,
) {
  const text = args[0] as string,
    tenantInput = args[1] as Record<string, string>;
  switch (operation) {
    case "getTenantStorage":
      return getTenantStorage(headers, text);
    case "manageTenantStorage":
      return manageTenantStorage(headers, text, tenantInput);
  }
  throw Error("Unknown storage operation");
}
export function publicError(error: unknown): PublicOperationError | undefined {
  if (error instanceof StorageUsageError)
    return { kind: "StorageUsageError", message: error.message, status: 400 };
}
