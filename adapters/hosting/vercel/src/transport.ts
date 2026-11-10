import {
  HostingProviderError,
  type VercelJSONTransport,
} from "@spitzli/hosting-provider-contracts";
type Fetch = (...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>;
/** Credentials remain explicit per call; fixed origin, bounded responses and no redirect forwarding. */
export function createVercelJSONTransport(
  fetcher: Fetch = (...args) => fetch(...args),
): VercelJSONTransport {
  return async function vercelJSON(
    path: string,
    token?: string,
    teamID?: string,
    init: RequestInit = {},
  ) {
    const url = new URL(path, "https://api.vercel.com");
    if (url.origin !== "https://api.vercel.com")
      throw new HostingProviderError(400, "Invalid provider endpoint.");
    if (teamID) url.searchParams.set("teamId", teamID);
    try {
      const r = await fetcher(url, {
        ...init,
        headers: {
          Accept: "application/json",
          ...(token ? { Authorization: "Bearer " + token } : {}),
          ...init.headers,
        },
        cache: "no-store",
        redirect: "error",
        signal: AbortSignal.timeout(10000),
      });
      if (!r.ok)
        throw new HostingProviderError(
          r.status === 403 ? 403 : 502,
          "Vercel could not complete this request. Check the connection and project permissions.",
        );
      const reader = r.body?.getReader();
      if (!reader) throw Error();
      const chunks: Uint8Array[] = [];
      let total = 0;
      try {
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          total += value.length;
          if (total > 2_000_000) {
            await reader.cancel();
            throw Error();
          }
          chunks.push(value);
        }
      } finally {
        reader.releaseLock();
      }
      return JSON.parse(Buffer.concat(chunks).toString()) as Record<
        string,
        unknown
      >;
    } catch (e) {
      if (e instanceof HostingProviderError) throw e;
      throw new HostingProviderError(
        502,
        "Vercel is unavailable. Check the current state before retrying.",
      );
    }
  };
}
