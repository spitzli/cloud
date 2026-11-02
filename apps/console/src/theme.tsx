import { useEffect, useState } from "react";
import { MonitorIcon } from "@phosphor-icons/react/dist/csr/Monitor";
import { MoonIcon } from "@phosphor-icons/react/dist/csr/Moon";
import { SunIcon } from "@phosphor-icons/react/dist/csr/Sun";
import { button } from "./ui";
export type ThemePreference = "system" | "light" | "dark";
const key = "webdock.theme.v1";
// Absent in some embedded and test environments; the light theme is the safe fallback.
const dark = () => (typeof window.matchMedia === "function" ? window.matchMedia("(prefers-color-scheme: dark)") : null);
export function readTheme(): ThemePreference {
  try {
    const value = localStorage.getItem(key);
    return value === "light" || value === "dark" ? value : "system";
  } catch {
    return "system";
  }
}
export function applyTheme(preference: ThemePreference) {
  document.documentElement.dataset.theme = preference === "dark" || (preference === "system" && dark()?.matches === true) ? "dark" : "light";
}
/** Applied before the first render on every surface; "system" follows the OS while the page is open. */
export function initializeTheme() {
  applyTheme(readTheme());
  dark()?.addEventListener("change", () => { if (readTheme() === "system") applyTheme("system"); });
}
const next: Record<ThemePreference, ThemePreference> = { system: "light", light: "dark", dark: "system" };
const labels: Record<ThemePreference, string> = { system: "System theme", light: "Light theme", dark: "Dark theme" };
export function ThemeButton() {
  const [preference, setPreference] = useState(readTheme);
  useEffect(() => {
    applyTheme(preference);
    try { localStorage.setItem(key, preference); } catch { /* Storage can be disabled by browser policy. */ }
  }, [preference]);
  const Icon = preference === "system" ? MonitorIcon : preference === "dark" ? MoonIcon : SunIcon;
  return <button className={button("quiet", "icon")} aria-label={`${labels[preference]}. Switch to ${labels[next[preference]].toLowerCase()}`} title={labels[preference]}
    onClick={() => { setPreference(next[preference]); }}>
    <Icon size={19} />
  </button>;
}
