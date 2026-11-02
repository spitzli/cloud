export function assertTestDatabase(raw: string): void {
  const url = new URL(raw);
  if (
    !["postgres:", "postgresql:"].includes(url.protocol) ||
    !["127.0.0.1", "localhost"].includes(url.hostname) ||
    url.pathname !== "/webdock_api_test" ||
    url.search ||
    url.hash
  )
    throw Error("Use the disposable loopback webdock_api_test database");
}
