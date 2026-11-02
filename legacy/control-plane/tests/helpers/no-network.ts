/** Test-process guard. Real provider calls are forbidden; tests must inject HTTP fixtures. */
const transport = globalThis.fetch.bind(globalThis);
globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
  const url = new URL(input instanceof Request ? input.url : String(input));
  if (!["127.0.0.1", "localhost", "[::1]"].includes(url.hostname))
    throw Error(
      "External network disabled in API tests; inject a provider fixture",
    );
  return transport(input, {
    ...init,
    redirect: init?.redirect === "manual" ? "manual" : "error",
  });
}) as typeof fetch;
