export function formText(data: FormData, name: string): string {
  const value = data.get(name);
  if (value === null) return "";
  if (typeof value !== "string") throw new Error("Expected a text form field.");
  return value;
}
