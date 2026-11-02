export const relatedID = (value: string | { id: string }) =>
  typeof value === "string" ? value : value.id;
export const date = (value: string, locale = "en") =>
  new Intl.DateTimeFormat(locale === "de" ? "de-DE" : "en-GB", {
    day: "numeric",
    month: "short",
    year: "numeric",
  }).format(new Date(value));
export const hostname = (value?: string | null, fallback = "No domain yet") => {
  try {
    return value ? new URL(value).hostname : fallback;
  } catch {
    return fallback;
  }
};

export const validRecordID = (id: string) =>
  /^[1-9][0-9]{0,18}$/.test(id) && BigInt(id) <= 9223372036854775807n;
