# Spitzli Cloud: TODO für den vollständigen Review

Bitte einmal das gesamte Projekt durchgehen: **Performance, Latenz, Security, potenzielle Bugs und Codequalität**. Ziel ist eine belastbare Liste der Dinge, die vor der Einladungs-Beta beziehungsweise vor einem breiteren Produktionsstart verbessert werden müssen.

Das hier ist der Prüfauftrag. Die Checkboxen sind offen; sie behaupten keine bereits abgeschlossene Gesamtprüfung oder Produktionsfreigabe.

## Verifizierung dieses Dokuments (2026-11-16)

Geprüfter Stand: `main` @ `531ea7a1` (Merge #107), Arbeitsbaum bis auf diese Datei sauber. Nur Abgleich der Referenzen und Annahmen mit dem Repository – kein inhaltlicher Review.

- Bestätigt: `AGENTS.md`, `README.md`, `tools/plugin-ci/CORE-QUALITY.md`, `tools/plugin-ci/BLACKSMITH.md`, `docs/architecture/mail-deployment-decision.md` und `legacy/` (auth, console, control-plane, deploy, mail, …) existieren; die Mail-Formulierung in Abschnitt 4 entspricht der Entscheidung.
- Bestätigt (UI und Backend): Die Freigabe versendet keine Einladung. `crates/access/src/waitlist.rs` setzt bei der Freigabe nur Status, Reviewer und Version (Operator-only, CAS über `version`) und schreibt ein Audit-Ereignis, löst aber keinen Mail- oder Einladungsversand aus. Die Console verweist danach auf die Mitgliederverwaltung. Ein E2E-Nachweis fehlt weiterhin.
- Neu: Alle drei Workflows (`quality.yml`, `backend-bootstrap.yml`, `blacksmith-qualification.yml`) laufen nur, wenn Repo-Variablen wie `SPITZLI_BLACKSMITH_READY`/`SPITZLI_BLACKSMITH_REGION` gesetzt sind. Ein „grüner“ Commit kann daher ein übersprungener Job sein (siehe Abschnitt 5). Der letzte in `BLACKSMITH.md` dokumentierte vollständige Quality-Lauf stammt vom 2026-11-04 (Commit `d370a9ca`). Für den aktuellen HEAD liegt im Repo kein Laufnachweis vor.
- Neu: Commit `04370cd1` verlangt EU-Builder für produktive native Releases; die US-West-Ausnahme gilt nur für das eigene CI (Abschnitt 5 präzisiert).
- Neu: Laufende Umbenennung Webdock → Spitzli (`dbde2b8a`, `24a72dac`, `05ce4d0d`). Außerhalb von `legacy/`, `vendor/` und `node_modules` enthalten noch rund 1.700 Dateien „webdock“, darunter produktive DB-Schemas (`webdock_auth.waitlist_entry`, `webdock_mail.service`), Adapter und CMS-Skripte. Das ist kein Bug an sich, aber die Konsistenz und die Migrationssicherheit einer späteren Schema-Umbenennung müssen geprüft werden.
- Neu: Website und Managed CMS laufen auf Vinext (`8f6aca83`); Next.js-Annahmen nicht ungeprüft übertragen (siehe Hinweis in `AGENTS.md`).
- Neu: Eigene Account-Identität, native Passwort-Anmeldung und Developer-/MCP-Zugang (`c9ab586b`, `5c9f198d`) erweitern die Auth-Angriffsfläche (Abschnitt 3 ergänzt).

## 0. Umfang und Ausgangslage

- [ ] `AGENTS.md`, `README.md`, relevante Architektur-/Betriebsentscheidungen und `tools/plugin-ci/CORE-QUALITY.md` lesen. Aktive Einstiegspunkte von historischen `legacy/`-Pfaden unterscheiden.
- [ ] Den tatsächlich geprüften Stand festhalten: Commit, uncommittete Änderungen, Source-Snapshot, Konfiguration und gegebenenfalls ausgelieferte Artefakte. GitHub-HEAD, Arbeitsbaum und laufende Version nicht gleichsetzen.
- [ ] Alle relevanten Bereiche inventarisieren: Rust Core/API, Identity/OAuth, Mandanten-/Projektverwaltung, Datenhaltung, Plugin-Host/-Runtime, Agent/Agent-Control, Build-/Deployment-Pipeline, Console, öffentliche Website sowie Hosting, Datenbanken, Storage, CMS, Analytics und Mail.
- [ ] Separat veröffentlichte Plugins, SDKs und CMS-Engine anhand ihres eigenen Source-/Release-Stands prüfen. Aus dem Monorepo allein keine Aussage über ein anderes Repository ableiten.
- [ ] Laufende Builds und eingefrorene Snapshots respektieren. Zunächst lesend prüfen; Fixes anschließend als kleine, nachvollziehbare Änderungen mit passenden Regressionstests umsetzen.

## 1. Performance

- [ ] Repräsentative Seiten und Aktionen auswählen. Warm-/Kaltstart, realistische Datenmengen und Parallelität messen; Umgebung, Stichprobengröße, Durchsatz, Fehlerrate, CPU, RAM und Verbindungen dokumentieren.
- [ ] Console und Website profilieren: initiale JS-/CSS-Menge, Lazy Loading, Renderkosten, unnötige Requests, Request-Wasserfälle, große Listen, Pagination, Bilder/Fonts, Cacheverhalten und relevante Web Vitals.
- [ ] Datenbankzugriffe auf N+1, unbeschränkte Abfragen, unnötige Spalten, fehlende oder unwirksame Indizes, teure Sortierungen, Pool-Erschöpfung und lange Locks prüfen. Auffällige Abfragen mit Query-Plänen und realistischen Fixtures belegen.
- [ ] Rust-/Bun-/Node-Pfade auf blockierende Arbeit, übermäßige Serialisierung/Kopien, große Payloads, Speicherwachstum und Ressourcenlecks prüfen. Uploads, Streams, Subscriptions, Hintergrundjobs und deren Cleanup einbeziehen.
- [ ] Queue-Verarbeitung und Worker unter begrenzter Last prüfen: Backpressure, Fairness, Ressourcenlimits, Rückstau und Verhalten bei langsamen oder ausgefallenen Abhängigkeiten.

## 2. Latenz

- [ ] Für wichtige Abläufe p50/p95/p99 und Fehlerquoten erfassen. Netzwerk-/Service-Zeit, Queue-Wartezeit und eigentliche Ausführungszeit getrennt ausweisen; Zielwerte erst mit einer dokumentierten Baseline begründen.
- [ ] Langsame Aktionen vollständig verfolgen: Browser → Gateway → Auth/Core → Datenbank → Plugin/RPC → Agent → Provider → Callback → sichtbarer UI-Zustand. Trace-/Request-IDs über die Grenzen hinweg prüfen.
- [ ] DNS, TLS/mTLS-Verbindungsaufbau, Wiederverwendung von Verbindungen, wiederholte Berechtigungsprüfungen und unnötig serielle Aufrufe untersuchen.
- [ ] Timeouts, Deadlines, Abbruch, Polling, Retries und Backoff aufeinander abstimmen. Insbesondere prüfen, ob ein HTTP-Timeout externe Arbeit wirklich beendet oder ein unbekanntes Ergebnis hinterlässt.
- [ ] Prüfen, ob Transaktionen oder Locks während externer I/O offenbleiben. Lange Sperren, Deadlocks, fehlende Cancellation und vermeidbare Rundreisen gezielt nachweisen.

## 3. Security

- [ ] Mandanten-/Projektgrenzen, Operator- und Kundenrollen, API-Tokens, Preview-/Read-only-Modus und feingranulare Rechte mit Negativfällen testen. Fremde, deaktivierte und gelöschte Ressourcen einbeziehen.
- [ ] Autorisierung vor und nach asynchronen/externalen Aufrufen prüfen. Widerruf, Rollenwechsel, Sitzungsablauf und Cache-Invalidierung dürfen keine veralteten Rechte weiterwirken lassen.
- [ ] SQL-Rollen, Objektbesitz, FORCE RLS, Spaltenrechte und SECURITY-DEFINER-Funktionen prüfen: feste Suchpfade, minimale EXECUTE-Rechte, NULL-/fehlender-Kontext-Denial und keine überprivilegierten Runtime-Verbindungen.
- [ ] Login, Einladungen, Aktivierung, Reset, Logout, MFA/Passkeys und OAuth/OIDC vollständig durchgehen: PKCE, State/Nonce, Issuer/Audience, Redirects, Token-Ablauf, Replay und Widerruf.
- [ ] Native Passwort-Anmeldung (Hashing-Parameter, Brute-Force-Schutz, Enumeration) sowie Developer-/MCP-Zugang der Accounts prüfen: Token-Scope, Bindung an Account/Projekt, Ablauf, Widerruf und Abgrenzung zum Clerk-Pfad.
- [ ] Eingabevalidierung, Größen-/Zeitlimits, SSRF/Egress, SQL-Injection, XSS, Pfad-/Archivbehandlung und Uploads prüfen. Traversal, Symlinks, Hardlinks, Dateirechte und fremde Dateieigentümer berücksichtigen.
- [ ] Cookies, CORS, CSRF, CSP sowie TLS/mTLS prüfen; falsche CA, Hostnamen und gleich signierte, aber nicht autorisierte Peers müssen scheitern. Schutzmaßnahmen nicht für einen grünen Test abschalten.
- [ ] Secrets, Provider-Credentials und private Schlüssel dürfen nicht in Browser-Bundles, Logs, Fehlermeldungen, Prozessargumenten oder Prüfberichten landen. Scope, Speicherung und Rotation prüfen, ohne Werte zu veröffentlichen.
- [ ] Abhängigkeiten, Container-/Release-Pins, Signaturen, veraltete Komponenten und tatsächlich erreichbare Angriffsflächen prüfen. Schwachstellenmeldungen nach Erreichbarkeit und Auswirkung bewerten.
- [ ] Öffentliche Waitlist-/Auth-Endpunkte auf Missbrauch, Account Enumeration, Rate Limits und parallele legitime Nutzer prüfen. Keine unkontrollierten Lasttests gegen Produktion.

## 4. Potenzielle Bugs und Datenkonsistenz

- [ ] Kritische Abläufe Ende zu Ende prüfen: Waitlist → einzelne Freischaltung → Einladung → Aktivierung/Login → Projektzugriff. Freigabe einer Waitlist-Anfrage darf nicht unbeabsichtigt eine Einladung versenden.
- [ ] Für Hosting, Datenbanken, Storage, CMS, Analytics und Mail jeweils erlaubte Lebenszyklen prüfen. UI, gewünschter Core-Zustand und tatsächlich laufender Dienst müssen übereinstimmen.
- [ ] Doppelrequests, verlorene Antworten, doppelte/veraltete Callbacks, Lease-Ablauf, Generationen, Idempotenz und Reconciliation testen. Unbekannte externe Ergebnisse dürfen weder dupliziert noch still als fehlgeschlagen behandelt werden.
- [ ] Quoten, Reservierungen und Jobs nach Fehlern prüfen: keine hängen gebliebenen Belegungen, doppelt berechnete Ressourcen, wiederbelebten zurückgezogenen Aufträge oder verlorenen Rückmeldungen.
- [ ] UI-Rennen prüfen: späte Antworten nach Projekt-/Accountwechsel, veraltete Caches, Dirty-/Uncertain-Formulare, Navigation, erneute Anmeldung, Offline-/5xx-Zustände und sicherer Retry.
- [ ] CMS-Entwürfe/Veröffentlichung, CAS-Konflikte, große Medienübertragungen und wiederholte Commits prüfen. Analytics-Zuordnung und Mail-Domain-/Sendergrenzen auf korrekte Projekt-/Kundenzuordnung testen.
- [ ] Backend-/Plugin-Migrationen mit bestehenden Daten prüfen: kompatible Verträge, erhaltene IDs/Rechte, atomare DDL, keine Teilmigration und ein tatsächlich zur neuen Schema-Version passender Recovery-Pfad.
- [ ] Ausfall einzelner Nodes, Datenbanken oder Provider prüfen. Abhängigkeiten und Single Points of Failure einschließlich Registry/Control-Plane erkennen; keinen Dienst durch eine leere Ersatzdatenbank scheinbar reparieren. Readiness mit einer kurzen echten Datenbankabfrage prüfen; volle Verbindungspools und hängende Start-/Anmeldeprozesse dürfen keinen gesunden Zustand melden. Wiederanlauf einschließlich abhängiger Dienste und kontrolliertem Neustart nachweisen.
- [ ] Backup und Restore praktisch nachweisen: Daten, Rollen, Credentials/PKI und erforderliche private Volume-Inhalte. Kunden-Workloads und Control-Plane-Sicherung getrennt bewerten; RPO/RTO, Alarmzustellung und Warteschlangen im Ausfall berücksichtigen.
- [ ] Deaktivierung, Suspend/Resume, Aufbewahrung und Löschung auseinanderhalten. Bei Mail die bestehende Entscheidung beachten: eingehend explizit aktivierte isolierte Stalwart-CE-Instanzen; outgoing-only ohne Stalwart.

## 5. Codequalität und Releasefähigkeit

- [ ] Fehlerbehandlung auf verschluckte Fehler, irreführende Erfolgsmeldungen, unklare Zustände und fehlende Diagnose prüfen. Ursache und Fehlerort müssen aus begrenzten, redigierten Logs erkennbar sein.
- [ ] Verträge zwischen Core, Agent, Plugins, Frontend und Datenbank prüfen: DTOs, Schema-/Protokollversionen, optionale Felder, Validierung und alle Konstruktoren/Test-Fixtures bei Änderungen. IDs bleiben verlustfreie Strings.
- [ ] Doppelte Logik, übergroße Module, unnötige Abstraktionen, tote Pfade, Konfigurationsreste und überholte Adapter bewerten. Nur dort refaktorieren, wo ein konkreter Nutzen oder Fehler belegt ist; benötigte Kompatibilität erhalten.
- [ ] Tests auf reale Aussagekraft prüfen: kritische Negativfälle, Integrationen und Regressionen tatsächlich ausgeführt? Null ausgeführte/gefilterte Tests, reine Parserprüfungen oder synthetische UI-Antworten sind kein Live-Nachweis.
- [ ] Reproduzierbarkeit der Builds prüfen: vollständige Workspace-/Datei-Closure, gesperrte lokale Vendor-Pakete und Patches, Dateirechte, echte Tool-Versionen, Source-/Artefakthashes und unveränderte Locks. Keine fremden kompilierten Caches als neuen Source-Nachweis übernehmen.
- [ ] Dokumentation, Beispiele, Konfigurationsschemas und tatsächlich ausgeliefertes Verhalten abgleichen. Code im Repository, freigegebene Features und produktive Verfügbarkeit getrennt ausweisen.
- [ ] CI-/Betriebsvorgaben berücksichtigen: eigenes GitHub-CI auf Blacksmith (US-West-Ausnahme nur dafür); produktive native Releases verlangen EU-Builder (`tools/plugin-ci/test_native_release_region.py`); Kundenleistungen nach den EU-Vorgaben. Die Public-Docs-Ausnahme nicht auf Kundendaten oder andere Dienste übertragen.
- [ ] Prüfen, ob die CI-Jobs tatsächlich laufen: Die Workflows sind über `SPITZLI_BLACKSMITH_*`-Variablen gegated. Übersprungene Jobs nicht als bestandene Qualitätsprüfung werten; Runner-/Regionsnachweis pro Lauf erfassen.
- [ ] Umbenennung Webdock → Spitzli auf Restbestände prüfen: Env-Namen, DB-/Schema-Bezeichner, Doku, Paket-/SDK-Exporte; Migrationen dürfen bestehende Daten und Rechte nicht verlieren.
- [ ] Kleine konkrete Fixes priorisieren. Nach jedem Fix den passenden Regressionstest und bei Performanceänderungen eine vergleichbare Vorher-/Nachher-Messung liefern.

## Ergebnis, das am Ende vorliegen soll

1. **Kurze Gesamtbewertung:** wichtigste Launch-Blocker und die nächsten sinnvollen Schritte.
2. **Befundliste** mit ID, Priorität (P0/P1/P2/P3), betroffener Datei/Zeile bzw. Dienst, Reproduktion/Nachweis, Auswirkung, kleinstem sinnvollen Fix und Akzeptanztest.
3. **Messübersicht** für Performance/Latenz mit Umgebung, Lastprofil, Perzentilen, Fehlerrate und Ressourcenverbrauch.
4. **Priorisierte Fix-TODO** mit Aufwandsschätzung, Abhängigkeiten und überprüfbarer Definition of Done.
5. **Explizite Lücken:** Was wurde nicht geprüft, was ist nur ein Verdacht und was ist durch Infrastruktur/Zugriff blockiert?

Bestätigte Fehler, begründete Verdachtsfälle und ungeprüfte Bereiche klar trennen. Vorhandene Build-/Restore-/UI-Nachweise nur für ihren tatsächlichen Stand und Umfang verwenden. Keine pauschale „sicher“, „schnell“ oder „prod-ready“-Aussage aus einzelnen grünen Tests ableiten.

## Rahmen

Produktive Kundendaten, reale externe Mail-/Provider-Effekte und destruktive Tests bleiben außerhalb sicherer Fixtures ohne passende ausdrückliche Freigabe. Keine Secrets oder sensiblen Betriebsdaten in dieses Dokument oder spätere Review-Ausgaben kopieren.
