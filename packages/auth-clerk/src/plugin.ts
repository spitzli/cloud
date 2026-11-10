import type { TrustedAuthPlugin } from "@spitzli/core/auth-plugin";
import { createClerkAdapter } from "./index";
export const manifest = { id: "auth-clerk", kind: "authentication" } as const;
/** Host supplies durable local mappings and provider-neutral local identity reads. */
export function createPlugin(dependencies: Parameters<typeof createClerkAdapter>[0]) {
  const adapter = createClerkAdapter(dependencies);
  return { manifest, ...adapter } satisfies TrustedAuthPlugin;
}
