/** Resource IDs stay decimal strings, including values beyond JavaScript's safe integer range. */
function decodeSearchValue(value: string): string {
  if (!value.startsWith('"')) return value;
  try {
    const parsed: unknown = JSON.parse(value);
    return typeof parsed === "string" ? parsed : value;
  } catch {
    return value;
  }
}
export function parseConsoleSearch(raw: string): Record<string, string> {
  const params = new URLSearchParams(raw.startsWith("?") ? raw.slice(1) : raw);
  const result: Record<string, string> = {};
  for (const key of new Set(params.keys())) {
    const values = params.getAll(key);
    const value = values[0];
    if (values.length !== 1 || value === undefined) continue;
    Object.defineProperty(result, key, {
      value: decodeSearchValue(value),
      enumerable: true,
      configurable: true,
    });
  }
  return result;
}
