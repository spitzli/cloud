// Controlled HTTP harness: the actual production SessionGate, no native/authentication claim.
import { useLayoutEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@webdock/i18n/react";
import type { Locale } from "@webdock/i18n";
import { ApiClient } from "../src/api";
import { ApiContext, useSession } from "../src/context";
import { ManagedDatabases } from "../src/ManagedDatabases";
import { ConsoleSessionGate } from "../src/ConsoleSessionGate";
import { DatabaseBackupIntentProvider, useBackupRequestMemory } from "../src/database-backup-intents";
import "../src/styles.css";
const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
const api = new ApiClient({ apiOrigin: location.origin, authOrigin: location.origin, enabledFeatures: ["databases"] });
declare global { interface Window { backupProofCache: () => string; backupProofIntent: () => string } }
window.backupProofCache = () => JSON.stringify([client.getQueryCache().getAll().map(q => q.state), client.getMutationCache().getAll().map(m => m.state)]);
function IntentProbe() { const memory = useBackupRequestMemory("10", "20", "1"); useLayoutEffect(() => { window.backupProofIntent = () => JSON.stringify(memory.cell.snapshot()); }, [memory.cell]); return null; }
function FixtureShell() { const actor = useSession(); return <><output data-fixture-role>{actor.user.preview ? "preview" : actor.user.role}</output><IntentProbe /><ManagedDatabases customerID="10" projects={[{ id: "2", name: "Project 2" }, { id: "20", name: "Project 20" }]} operator={actor.user.role === "operator"} /></>; }
function Harness() {
  const [role, setRole] = useState("operator"), [locale, setLocale] = useState<Locale>("en");
  const changeRole = async (value: string) => { setRole(value); await fetch("/__backup-proof-control", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ sessionRole: value }) }); await client.invalidateQueries({ queryKey: ["session"] }); };
  return <I18nProvider locale={locale} preference={locale}><QueryClientProvider client={client}><ApiContext.Provider value={api}><DatabaseBackupIntentProvider>
    <main className="mx-auto max-w-5xl p-4 sm:p-6"><p>Controlled HTTP fixture only — no Core or Native proof</p><label>Fixture actor <select aria-label="Fixture actor" value={role} onChange={e => { void changeRole(e.target.value); }}><option>operator</option><option>member</option><option>preview</option></select></label> <label>Fixture language <select aria-label="Fixture language" value={locale} onChange={e => { setLocale(e.target.value === "de" ? "de" : "en"); }}><option>en</option><option>de</option></select></label><ConsoleSessionGate><FixtureShell /></ConsoleSessionGate></main>
  </DatabaseBackupIntentProvider></ApiContext.Provider></QueryClientProvider></I18nProvider>;
}
const root = document.getElementById("root"); if (!root) throw new Error("Fixture root missing"); createRoot(root).render(<Harness />);
