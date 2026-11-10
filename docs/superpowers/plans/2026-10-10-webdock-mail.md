# Webdock Mail: Umsetzungsfahrplan

> Für ausführende Agenten: Konzept zuerst lesen. Für die Ausführung `superpowers:executing-plans` verwenden. Dieser Fahrplan wird paketweise umgesetzt; offene Integrationsnachweise werden vor dem abhängigen Produktcode erbracht. Die Umsetzung ist seit 2026-10-10 autorisiert; die [CE-Deploymententscheidung](../../architecture/mail-deployment-decision.md) überschreibt frühere Enterprise-Annahmen.

**Ziel:** Native Webdock-Mailplattform mit verpflichtendem Managed-Versand, Enterprise-Turbo-BYOK, Stalwart-Postfächern, Weiterleitungen und Webmail im ersten Kundenrelease.

**Architektur:** Vorhandenes Studio und Better Auth bleiben Oberfläche und Identitätsinstanz. Ein kleiner dauerhafter Maildienst übernimmt Versand-/JMAP-Integration und Worker. Nur Mandanten mit aktiviertem Empfang erhalten Stalwart für Mailprotokolle, Mailboxinhalte und Zustellqueue. Reiner Versand läuft Webdock API → Turbo Relay-API; mit Empfang Webdock API → Stalwart → Turbo; Turbo ist das gewählte externe Relay.

**Stack:** vorhandenes Node 24, TypeScript, PostgreSQL/`pg`, Better Auth, Next.js 16.3.8 und React; Stalwart in fest gepinnter Version; bestehendes Nodemailer für MIME/SMTP wiederverwenden, wo passend.

**Konzept:** [Native Webdock Mail](../specs/2026-10-10-webdock-mail-design.md).

**Protokoll- und Funktionsgrundlage:** [Stalwart-Gesamtprüfung](../../architecture/stalwart-capabilities.md), einschließlich CalDAV/CardDAV, JMAP-Erweiterungen, WebDAV, Freigaben, Serverautomatisierung und Backupgrenzen.

**Planungstiefe:** Arbeitspakete, Grenzen, betroffene Dateien und konkrete Abnahmeszenarien. Für die noch unbewiesenen Stalwart-/Turbo-Schnittstellen werden keine erfundenen ausführbaren Konfigurationen vorgegeben. Paket 0 erzeugt diese versionierten Verträge und Tests; erst danach werden die jeweiligen Codeänderungen detailliert.

## Globale Vorgaben

- Stalwart CE: eine isolierte Instanz nur pro Mandant mit ausdrücklich aktiviertem Mailempfang (Postfächer oder Weiterleitung). Reiner Versand provisioniert keine Instanz. Deaktivierung löscht keine Daten.
- Produktname Webdock Mail; normales Kundenprodukt ohne Relaykit-/Turbo-Zugänge oder Providerfehler.
- Managed ist für normale Kunden verbindlich. BYOK und Direktbetrieb sind serverseitige Enterprise-Berechtigungen.
- Webmail ist Bestandteil des ersten Kundenreleases.
- Bestehendes `customer_id`, vorhandene Rollenprüfung und Snowflake-Dezimalstrings verwenden.
- Kein Zugriff auf Postfachinhalte aus bloßer Operator-/Mandantenadminrolle.
- Jede externe Nachricht bleibt an Mandant und Transportrevision gebunden. Kein stiller BYOK-Fallback.
- Unklare Versandannahme niemals automatisch wiederholen. Übergabe an Turbo nicht als endgültige Zustellung ausweisen.
- Mailbox- und MIME-Inhalte bleiben privat. Keine Nutzung des öffentlichen CMS-Blobstores.
- Bestehende Daten/Mappings erhalten; alte direkte Zugangsdaten erst nach erfolgreicher Umstellung gezielt widerrufen.
- Produktcode erst nach Lesen der passenden lokalen Next-Guides unter `node_modules/next/dist/docs/`; im untersuchten Worktree waren diese Dependencies noch nicht installiert.
- `RTK.md` war in den geprüften Projekt-/Vorfahrenpfaden nicht vorhanden. Bei Ausführung erneut auf eine inzwischen verfügbare Datei prüfen.
- Tests mit isoliertem lokalen PostgreSQL und Stalwart-Testdeployment. Vorhandene Preview-/Produktivdatenbanken sind keine Testumgebung.

## Besondere Prüffälle

| Risiko | Verantwortliches Paket |
| --- | --- |
| Mandant A setzt From, Blob-ID oder Mailbox-ID von B | 0, 1, 3, 4, 5 |
| Timeout nach SMTP-Annahme, Worker-Neustart, früher Callback | 0, 3, 7 |
| Weiterleitung mit fremdem Absender, Null-Sender und Schleife | 0, 6 |
| Benutzerentzug während Sitzung/Download; Mitglied in zwei Mandanten | 0, 4, 5 |
| Umschalten BYOK bei gefüllter Queue und alten direkten Schlüsseln | 2, 7, 8 |

## Ausführungsstand

2026-10-10: CE-Deploymententscheidung festgehalten; isoliertes Zwei-Instanzen-Labor, defensiver JMAP-Managementclient und reproduzierbarer Testlauf implementiert. Unit 7/7, reale CE-Integration 3/3, TypeScript erfolgreich. Paket 0 bleibt wegen der weiteren Integrationsnachweise offen. [Nachweise und verbleibende Grenzen](../../architecture/mail-integration-evidence.md).

## Paket 0: Integrationsnachweise und endgültige technische Verträge

**Effort:** High. Abhängigkeit: keine. Ergebnis ist ein ausführbarer isolierter Testaufbau und ein kurzer Nachweisbericht, noch kein Kunden-Rollout.

**Neue Dateien:**

- `infra/mail/compose.test.yml`: isoliertes Stalwart, Test-SMTP-Sink, private Volumes, lokales Testnetz.
- `infra/mail/README.md`: gepinnte Images/Digests, Ports, Edition, Lizenzzählung, Start/Stop und Datentrennung.
- `docs/architecture/mail-integration-evidence.md`: getestete Version, API-Verträge und tatsächliche Ergebnisse.
- `apps/mail/package.json`: zunächst nur privates Testpaket mit den benötigten Integrationstest-/Typecheck-Skripten, noch ohne Produktserver.
- `apps/mail/tests/integration/`: Transport-, Policy-, JMAP-, Event- und Forward-Proben mit synthetischen Daten.

**Schritte / Abnahme:**

- [ ] Zwei unabhängige Stalwart-CE-Instanzen mit getrennten Stores, Domains, Benutzern und Postfächern erzeugen. Cross-Tenant-JMAP, Suche, Quota und Domainübernahme müssen scheitern.
- [ ] Zwei unterschiedliche Relay-Sinks einsetzen. API-/SMTP-Testsendung von A erreicht nur Sink A; gefälschtes From von B wird abgewiesen. Ein Retry behält nach Konfigurationswechsel die alte Revision.
- [ ] Routing anhand tatsächlich verfügbarer Queue-Metadaten beweisen. Die genaue Envelope-/Routenabbildung im Nachweisbericht festhalten.
- [ ] Stalwart-Hook für Envelope/Header, Budget und korrekte Mandantenherkunft testen. JMAP-Submission/Sieve/Forward ausdrücklich separat prüfen.
- [ ] Authentifizierte Stalwart-Ereignisse mit Queue-Referenz aufzeichnen. Verlorene Callbacks durch CE-Queue-/eigenen Event-Abgleich erkennen.
- [ ] Mit freigegebenem Testkonto exakte SMTP→Turbo-Korrelation ermitteln; ein Live-Test darf nicht aus der bloßen Konfiguration als bestanden gelten. Zugelassene reale Testadressen aus der Relaykit-Quelle: `noreply@spitzli.dev` → `dominik@spitzli.dev`. Kein Bestands-Callback wird dafür überschrieben.
- [ ] Better-Auth-Mail-Audience, stabile Mailidentität, zwei Mandanten desselben Benutzers und geteiltes Postfach prüfen. Kein automatisch freigeschalteter Benutzer beim ersten Tokenkontakt.
- [ ] Postfach vor Erstlogin empfangsbereit; nach Entzug keine neuen Webmail-/IMAP-/SMTP-Zugriffe. Native Tokens, App-Passwörter und aktive Sessions berücksichtigen.
- [ ] Transparente und gekapselte Weiterleitung gegen synthetische DKIM-/DMARC-Fälle, Bounce, Null-Sender und Schleife prüfen. Reale Provider-Kompatibilität getrennt dokumentieren.
- [ ] Teilrestore durchführen, CE-Funktionsumfang und Ressourcenbedarf je Instanz prüfen.
- [ ] JMAP-Session und API-Schema der gepinnten Version aufnehmen. Nicht freigegebene Kalender-/Kontakte-/Datei-/Sieve-Methoden über JMAP und DAV testen: serverseitige Ablehnung trotz erreichbarem HTTP-Listener.
- [ ] Alle serverseitigen Mailquellen inventarisieren: iMIP, Kalenderalarme, Vacation, Sieve-Notify, DSN und Forward. Route/Budget festlegen, sonst betreffende Capability ausgeschaltet lassen.

**Handoff:** Bericht enthält konkrete Requests/Responses ohne Secrets, die freigegebene Konfiguration und eine Capability-Liste. Fehlender SMTP-Eventnachweis führt zu ausdrücklich eingeschränktem Zustellstatus oder einem separaten Egress-Adapter-Paket. Ein Scheitern ändert die technische Variante; es rechtfertigt kein unsicheres Durchwinken.

## Paket 1: Mandantenmodell, Rechte und additive Migration

**Effort:** High. Abhängigkeit: 0 für externe Referenzen; lokale Schemaarbeit kann nach Konzeptfreigabe unabhängig vorbereitet werden.

**Dateien:** Neue `apps/auth/scripts/migrate-mail-native.ts`, `apps/auth/src/lib/mail-access.ts`, `mail-entitlements.ts`; Anpassung von `tenant-mail.ts`, `plans.ts`, `platform.ts`, `studio-api.ts` und `apps/admin/src/lib/studio-contracts.ts`. Neues `packages/mail-core/src/` nur für tatsächlich gemeinsam genutzte Verträge/Validierung.

**Vertrag:** `customerID` bestimmt Ressourcenbesitz; `MailAccess` enthält Benutzer, Kunde und explizite Mailrechte. Ein Eingabeparameter `customerID` oder eine Mailbox-ID ist nie selbst ein Grant. Verwaltungsantworten enthalten keine Postfachinhalte oder Transportsecrets.

- [ ] Additive Tabellen aus Konzept Abschnitt 8 und explizite DB-Grants erstellen; Snowflake-Allocator weiterverwenden. Migration zweimal auf Testdatenbank ausführbar.
- [ ] Bestehende Managed-Unterkonten abbilden, ohne sofort den Sendepfad umzuschalten. Interner Migrationsstatus `legacy_direct` bleibt vom Kundenprofil getrennt.
- [ ] Enterprise-Rechte und getrennte Empfangs-/Versand-/Zugriffszustände implementieren. Plan-Downgrade erzeugt keinen Transportwechsel und löscht keine Inhalte.
- [ ] Mailboxrechte unabhängig von `getTenant(...).canManage` prüfen. Tests für Operator ohne Leserecht, Admin ohne Leserecht, Mitglied mit einem Grant und fremden Mandanten.
- [ ] `mail_identity` vom Postfachmodell trennen; Modul-/Protokollberechtigungen sind explizit. Account-/Tenant-Speicher als gemeinsames Ressourcenlimit abbilden, nicht als reines Mailbody-Limit.
- [ ] Zweckgebundene Secretverschlüsselung mit Key-ID ergänzen; vorhandene Credentials in überprüfbaren Schritten migrieren. Defekte Secrets ergeben Fehler, keine leere/Default-Route.

**Abnahme:** Bestandsverwaltung bleibt lesbar; kein neues Feature öffnet Zugriffe, bevor seine wirksame Konfiguration bereit ist.

## Paket 2: Stalwart-Provisionierung, Domains und Turbo-BYOK

**Effort:** High für Provisionierung/Routing, Medium für Formulare. Abhängigkeit: 0–1.

**Dateien:** Neue `apps/mail/src/stalwart/management.ts`, `provisioning.ts`, `transports.ts`, `domains.ts`; Erweiterung von `apps/auth/src/lib/mail-domains.ts`, `mail-dns.ts`, `turbosmtp.ts`, `platform.ts` und vorhandenen Studio-Mailseiten. Infrastruktur unter `infra/mail/`.

**Vertrag:** Gewünschte Konfiguration und bestätigte Konfiguration werden getrennt gespeichert. Jeder Vorgang hat eine Webdock-Operation-ID, externe Referenzen, Revision und Zustand `pending/running/ready/needs_review/failed`.

- [ ] Stalwart-CE-Instanz bei Mailaktivierung, Domain, DKIM, Serviceprincipal und Route anhand von Paket 0 provisionieren. Wiederholte Jobs erzeugen keine Duplikate.
- [ ] Managed-Unterkonto serverseitig verbinden. BYOK nur mit Berechtigung und verschlüsselten, minimal nötigen Credentials anlegen; nur bekannte Turbo-Hosts/Regionen als Ziele zulassen.
- [ ] Unbekannte/gelöschte Route testen: kein direkter MX-Fallback trotz Stalwart-Default. Egress auf bestätigte Relayziele beschränken, ohne erforderliche DNS/HTTPS-Dienste zu unterbrechen.
- [ ] Verbindungstest prüft Erreichbarkeit/Auth; DNS-/Sendebereitschaft separat prüfen. Eine erfolgreiche SMTP-Anmeldung beweist keine Domainfreigabe oder Zustellbarkeit.
- [ ] TXT-Besitzprüfung erhalten; Empfangsmodus nicht aus Versandregistrierung ableiten. DNS-Vorschläge bewahren bestehendes SPF/DMARC.
- [ ] Revisionswechsel mit wartenden Nachrichten testen. Falsches BYOK-Credential, Timeout und verlorener Response aktivieren keine neue Route; kein Managed-Fallback.
- [ ] Entfernung von Domains mit aktiven Postfächern, Forward-Regeln oder offenen Aufträgen blockieren bzw. durch einen ausdrücklichen geordneten Retirement-Vorgang ersetzen.

**Abnahme:** Zwei Kunden senden über getrennte Sinks; normale Kunden sehen ausschließlich Webdock-Daten. Empfang bei externem MX bleibt unverändert.

## Paket 3: Native Versand-API, SMTP-Regeln und Worker

**Effort:** High. Abhängigkeit: 1–2.

**Dateien:** `apps/mail/package.json` aus Paket 0 um Laufzeitbefehle ergänzen; neue `src/server.ts`, `src/worker.ts`, `src/api/`, `src/sending/`, `src/policy/`, `src/keys.ts` unter `apps/mail/`. Bestehende Auth-/Studio-Keyverwaltung umstellen; alle älteren Aktionen mitprüfen.

**Gezielte Quelle:** Relaykit `emails.ts`, `sending.ts`, `api.ts`, `keys.ts`, Simulator und zugehörige Tests. Kein Kopieren seines Auth- oder Datenbanksystems.

**Vertrag:** `POST /v1/emails` akzeptiert Webdock-Key, Nachricht und optionalen Idempotenzschlüssel und liefert `202` mit String-ID und `accepted/scheduled`. Ein GET liefert Status je Empfänger. Der Worker besitzt die einzige Übergabe vor Stalwart; Stalwart besitzt Retries nach Annahme.

- [ ] Gehashte `wd_mail_...`-Schlüssel mit Sendescopes, Domainbindung und Widerruf implementieren. Nach Domainlöschung wird ein eingeschränkter Schlüssel unbrauchbar, niemals unbeschränkt.
- [ ] Eingaben/MIME inklusive BCC, Unicode/IDN, CRLF-Injection, Inline-CID, maximaler Größe und ungültigen Daten prüfen. Envelope aus To/CC/BCC getrennt vom sichtbaren Header erzeugen.
- [ ] Durable Outbox, atomare Claim/Lease und private Payloadablage einbauen. Schlüssel+Request-Hash einschließlich Batch-Teilergebnissen speichern.
- [ ] Zeitplanung, Storno vor Übergabe und erneute Rechteprüfung beim Dispatch übernehmen. Widerruf sperrt noch nicht übergebene Jobs.
- [ ] SMTP-Zugang auf authentifizierte Webdock-Serviceprincipals begrenzen; Policy gilt auch bei Nutzung außerhalb der API.
- [ ] Atomic Budgetreserve am gemeinsamen Annahmepunkt; API-Vorreservierung beim SMTP-Schritt nur einmal konsumieren. Quota-Concurrencytests mit mehreren Workern.
- [ ] SMTP-Verbindung direkt nach DATA-Annahme unterbrechen: `submission_unknown`, auch nach Neustart keine erneute Übermittlung. Eindeutige Ablehnung vor Annahme muss unterscheidbar bleiben.
- [ ] Tests für Empfänger A unterdrückt/B zulässig, Bcc-only, partiellen Batch und falsche Mandantenheader. Simulator ist vollständig ohne echten Versand verwendbar.

**Abnahme:** Anwendungsintegration kann vollständig mit Webdock-Key/Host arbeiten. Kein direkter Turbo-Key ist für neue Managed-Kunden erzeugbar, auch nicht über alte Auth-Seiten/RPC.

## Paket 4: Postfachverwaltung und Mailidentität

**Effort:** High. Abhängigkeit: 0–2.

**Dateien:** Neue `apps/mail/src/mailboxes.ts`, `src/mail-sessions.ts`, `src/stalwart/jmap.ts`; Anpassungen in `apps/auth/src/lib/auth.ts`, `tenants.ts`, `mail-access.ts` und `studio-api.ts`; neue Studio-Unterseiten `mail/mailboxes/` und `mail/addresses/`.

**Vertrag:** Ein eigener OIDC-Mailkontext bzw. der in Paket 0 bestätigte Broker liefert benutzerbezogene Rechte. Management-Client und JMAP-Datenclient verwenden getrennte Credentials. Kein JMAP-Proxy mit globalem Admin-Key.

- [ ] Postfächer, Alias-Adressen, gemeinsame Postfächer und individuelle Grants provisionieren; bevor `ready`, Empfang mit synthetischer Nachricht prüfen.
- [ ] Benutzer A gehört zwei Kunden: Wechsel der Mailidentität darf keine Ordner, Tokens oder Attachments des anderen Kontextes übernehmen.
- [ ] App-Passwörter mit Nutzerkontext einmalig ausgeben, listen und widerrufen. Nur SMTP-Zugang benötigt keine Postfachleserechte.
- [ ] Membership-Entzug, Benutzerban und Mailboxentzug synchronisieren. Readback, Sessionentzug und Cache-Invalidierung testen; Fehler sichtbar nachverfolgen.
- [ ] Quotas auf tatsächlichen Stalwart-Speicher abbilden. Bei Downgrade unter vorhandenen Verbrauch: vorhandene Mails lesbar, definierte Annahmesperre, keine Löschung.

**Abnahme:** Ein neu provisioniertes Postfach empfängt vor dem ersten Login. Nur ausdrücklich berechtigte Personen können es lesen und unter seinen Adressen senden.

## Paket 5: Native Webmail, verpflichtend vor Kundenstart

**Effort:** Medium für Layout/Listen, High für JMAP, Rechte, HTML und Senden. Abhängigkeit: 3–4.

**Dateien:** Neue Studio-Unterseite `apps/admin/src/app/(console)/(portal)/tenants/[customerID]/mail/inbox/`, fokussierte Komponenten in `apps/admin/src/components/mail/`; neue `apps/mail/src/webmail/` für Ordner, Nachrichten, Entwürfe, Suche und begrenzte Upload-/Downloadpfade.

**Vertrag:** JMAP-IDs sind opake Providerreferenzen innerhalb eines autorisierten Accounts. Upload-/Blob-Berechtigungen schließen Kunde, Account, Benutzer, Größe und Ablauf ein. Der Versand erzeugt denselben Submission-Vertrag wie Paket 3, ergänzt um Postfach-/Entwurfsbezug.

- [ ] Postfachwechsel, Ordner, paginierte Liste, Lesen und Suche implementieren; Stalwart bleibt Datenquelle.
- [ ] SSE über den dauerhaften Maildienst, Changes-Abgleich und Polling-Fallback implementieren. Disconnect, ungültiger alter State und Grantentzug bei offener Verbindung testen.
- [ ] Verfassen, CC/BCC, Send-as, Reply/Reply-all, Weiterleiten und Entwurfs-Autosave mit JMAP-State-Konflikten ergänzen.
- [ ] Anhänge/CID, authentifizierte Downloads und Grenzen für finale MIME-Größe durchgängig testen. Keine großen Inhalte über vorhandenes `/api/studio` tunneln.
- [ ] HTML bereinigen/isolieren; Remote-Bilder aus; schädliche Formulare, Skripte, URLs, SVG-/HTML-Anhänge und bösartige Dateinamen prüfen.
- [ ] Gemeinsamen SMTP-Sendepfad nutzen; erfolgreiche Übergabe genau einmal, „Gesendet“-Ablage unabhängig reparierbar. Browserneuladen nach Timeout darf keine zweite Mail senden.
- [ ] Markieren, Verschieben, Archiv, Spam und Papierkorb; endgültiges Löschen bewusst bestätigen. Kein globaler Suchzugriff.
- [ ] E2E: A schreibt mit Anhang an B, B liest/antwortet in Webdock, Ordnerstatus stimmt auch über IMAP. Danach A den Grant entziehen; offene Ansicht, Download und neue Sendung müssen gesperrt sein.
- [ ] Mobile Darstellung und Tastaturbedienung im Browser prüfen. T3-Preview verwenden, falls verfügbar.

**Abnahme:** Ein Kunde erledigt den normalen Empfang-/Antwortablauf vollständig in Webdock ohne Stalwart- oder Turbo-Oberfläche.

## Paket 6: Weiterleitungen und Empfangsregeln

**Effort:** High. Abhängigkeit: 0, 2–4.

**Dateien:** Neue `apps/mail/src/forwarding.ts`, `src/forwarding-verification.ts`, Tests und versionierte Stalwart-Regeln unter `infra/mail/`; Studio-Unterseite `mail/forwarding/`.

**Vertrag:** Der Besitzer der empfangenen Adresse finanziert und autorisiert die Weiterleitung. Externes Ziel ist erst nach Bestätigung aktiv. Modus `transparent` benötigt Capability-Nachweis, `encapsulated` ist ausdrücklich gekennzeichnet.

- [ ] Zeitlich begrenzte Bestätigungstokens gehasht speichern; Rate-Limit gegen Bestätigungsmail-Missbrauch. Zieländerung invalidiert alte Bestätigung.
- [ ] Reine Weiterleitung und Postfach+Kopie implementieren; ohne Postfach nur notwendige Queue-/Quarantänedaten halten.
- [ ] Gekapselte Mail mit eigenem verifiziertem Absender und Original-`.eml` bauen. Originalinhalt nicht als aktive HTML-Oberfläche behandeln; Reply-To streng parsen.
- [ ] Transparenten Modus nur nach Paket-0-Nachweis freischalten; fremdes DKIM nicht durch Tracking zerstören.
- [ ] Zyklen A→B→A, externe Wiederkehr, mehrere Mandanten als Empfänger, unbekanntes Ziel, Null-Sender, Autoresponder, Ziel-Bounce und Spam prüfen.
- [ ] Fanout/Quotas und Sperren auch bei serverseitigen Regeln durchsetzen. Kunde kann keine unkontrollierten Sieve-Redirects am Produktbudget vorbei anlegen.

**Abnahme:** Weiterleitung ohne reguläres Postfach funktioniert nachvollziehbar; Providerbeschränkungen werden nicht durch einen unsichtbaren anderen Versandweg umgangen.

## Paket 7: Ereignisse, Kunden-Webhooks, Nutzung und Betrieb

**Effort:** Medium für UI, High für Eventkorrelation/Abrechnung. Abhängigkeit: 3, 6.

**Dateien:** Neue `apps/mail/src/events/`, `webhooks/`, `usage.ts`, `retention.ts`; neue Studio-Mailseiten für Aktivität/Webhooks; neue `docs/architecture/mail-operations.md`. Auth-Planansichten und Studio-Verträge ergänzen.

**Gezielte Quelle:** Relaykit `delivery-events.ts`, `webhook-signing.ts`, `webhook-transport.ts`, `webhooks.ts`, `suppressions.ts`, `log-redaction.ts` und ihre Tests.

- [ ] Ereignisse zuerst dauerhaft speichern; deduplizierte Empfängerübergänge und Webhook-Outbox in einer Transaktion. Late events verändern nicht die falsche Nachricht/Route.
- [ ] Vorzeitig eintreffenden Callback, falsches Konto, fremde Provider-ID und fehlende Referenz testen. Unzuordenbare Events isoliert halten, nicht raten.
- [ ] Stalwart-Telemetrieausfall simulieren und Nachabgleich ausführen. Aufbewahrungs-/Discard-Fenster dokumentieren.
- [ ] Bestehenden Turbo-Callback-Verbraucher erhalten oder expliziten Fanout installieren; BYOK-Callbacks isolieren.
- [ ] Kunden-Webhooks mit verschlüsseltem Secret, HMAC, DNS-Pinning, Redirectverbot, Lease, Backoff und begrenzten Responses portieren. SSRF/Replay/Rotation testen.
- [ ] Neue Verbrauchsmetrik ohne rückwirkende Umdeutung von `mailMessages`; Reservation/Reconciliation für API, SMTP und Forward. Direkten BYOK-Verkehr als extern verwaltet kennzeichnen.
- [ ] Retention-Jobs, Quarantänefristen, Queue-/Lizenz-/Disk-/Zertifikatsalarme und Mandantenrestore einrichten. Backup-Inhalte/Schlüssel auf tatsächliche Wiederherstellbarkeit prüfen.

**Abnahme:** Kundensicht unterscheidet Auftrag, Relay-Annahme und endgültige Zustellung. Eventlücken und veraltete Usage-Werte sind sichtbar statt fälschlich null/erfolgreich.

## Paket 8: Bestandsmigration und erster Kundenrelease

**Effort:** High. Abhängigkeit: 0–7 einschließlich Webmail.

**Dateien:** Neue `apps/auth/scripts/migrate-mail-customers.ts`, `docs/architecture/mail-cutover.md`; Überarbeitung von `docs/architecture/platform-mail.md`, `plans-and-storage.md`, betroffenen `.env.example`-Dateien und öffentlichen Mailbeispielen. Keine echten Secrets.

- [ ] Pro Tenant Inventarliste mit Altkeys, Anwendungen, Unterkonto, Domains, Tracking und offenen Aufträgen erzeugen.
- [ ] Interne Webdock-/Spitzli-Testmandanten durch den vollständigen Ablauf führen: DNS, API/SMTP, Eingang, Webmail-Antwort, Forward, BYOK, Backup/Restore.
- [ ] Neue Endpunkte/Keys in bekannten Mailverbrauchern umstellen, einschließlich Auth-/Website-/CMS-Transportkonfiguration, soweit sie den neuen Dienst nutzen sollen. Externe Kundenanwendungen benötigen eine explizite Cutoverliste.
- [ ] Altkeys nach erfolgreicher Umstellung widerrufen und Provider-Readback sichern. Erst dann Migration als abgeschlossen markieren.
- [ ] Mailboxmigration separat von API-Cutover durchführen; MX, Nachsynchronisation und Rollbackfenster dokumentieren.
- [ ] Falls Relaykit-Bestandskunden migriert werden: explizites Ressourcenaccount-Mapping, Schedules und Idempotenzschutz erhalten. Kein paralleles Dispatch beider Systeme.
- [ ] Auf einer Testumgebung Rollback bei laufender Queue üben. Keine doppelten Sendungen, keine Wiederherstellung widerrufener Credentials, keine übersehenen neuen Postfachdaten.
- [ ] Kundensichten/OpenAPI/Fehler nach Relaykit/Turbo durchsuchen; technische Operator-/BYOK-Informationen und historische Migrationsdokumentation bewusst ausnehmen.

**Releasekriterium:** Managed-, BYOK-Gateway- und BYOK-Direktregeln, native Webmail und Weiterleitungen sind getestet. Nicht bestandene Integrationsnachweise sind konkrete Launch-Blocker für die betroffene Fähigkeit und werden nicht als erledigt markiert.

## Empfohlener anschließender Ausbau: Kalender und Kontakte

Dieses Paket folgt aus der zusätzlich angefragten Stalwart-Prüfung. Seine native UI ist noch keine ausdrückliche Anforderung für den ersten Mailrelease; die oben ergänzten Sicherheits-/Datenmodelltests sind dagegen Teil des Fundaments.

**Effort:** High für Rechte, Sync und Scheduling, Medium für bestätigte UI-Abläufe. Abhängigkeit: Mailidentität, JMAP-Anbindung, Betrieb und gemeinsame Versandregeln.

**Dateien:** Neue `apps/mail/src/calendars/`, `src/contacts/` und entsprechende Studio-Unterseiten im Mandantenkontext. Bestehende Mailidentität, Gerätezugänge und JMAP-Anbindung erweitern; keine zweite Calendar-/Contact-Datenbank.

- [ ] CalDAV-/CardDAV-Discovery und ausdrücklich aktivierte Rechte bereitstellen; tatsächliche Apple-/Thunderbird-/Android-/Outlook-Varianten mit konkreter Version dokumentieren.
- [ ] Persönliche und geteilte Adressbücher, vCard-Import/Export, native Kontakte und berechtigte Composer-Vervollständigung aufbauen.
- [ ] Kalenderansichten, Ereignisse, Serienausnahmen, Teilnehmer, Zeitzonen, Einladungen/Updates/Absagen und Free/busy über die bestätigte JMAP-Kalenderversion umsetzen.
- [ ] iMIP/Erinnerungen durch dieselbe Managed-/BYOK-/Budgetkontrolle führen; Doppelversand durch Client und Server verhindern.
- [ ] Webdock-RSVP-/E-Mailvorlagen und tokengebundenen bewussten Teilnahme-POST verwenden. Linkscanner darf nicht antworten.
- [ ] Sync-Roundtrip JMAP→CalDAV/CardDAV→JMAP, konkurrierende Änderungen, Gruppenfreigaben, Entzug, Cross-Tenant-Discovery und Sommerzeit testen.
- [ ] Account-/Gruppenrestore mit Kalendern, Kontakten und ACLs durchführen. Firmware-/Client-Kompatibilität nicht aus allgemeinem Protokollsupport ableiten.

Private WebDAV-/JMAP-Dateiablage, SCIM-Kundenanbindung und Masked Email bleiben eigene spätere Pakete. Das ermöglicht den Ausbau auf demselben Stalwart-Fundament, ohne sie zum versteckten Aufwand des ersten Webmail-Releases zu machen.

## Verifikation bei Ausführung

Bestehende Projektprüfungen, nach Installation und sicherer Testkonfiguration:

```bash
npm run test:auth
npm test
npm run check -w @spitzli/auth
npm run build:auth
npm run build:admin
npm run lint --workspaces --if-present
```

Das neue Mailpaket definiert während seiner Erstellung eigene `test`, `test:integration` und `check`-Skripte; diese werden paketweise und vor Release ausgeführt. Dies sind zukünftige Prüfaufträge, keine in diesem Planlauf bereits bestandenen Tests. Externe DNS-/SMTP-/Restore-Nachweise werden separat protokolliert; gemockte Tests ersetzen sie nicht.

## Empfohlene Arbeitsteilung nach Denktiefe

| High | Medium |
| --- | --- |
| Paket 0, Tenant-/Mailboxrechte, OIDC, Secretmigration | Bereits spezifizierte Verwaltungsformulare und Texte |
| SMTP-Routing, Retry-/Idempotenzgrenzen, atomare Quoten | Listen, Filter, sichere Statusanzeigen und Quickstarts |
| HTML-/Anhangssicherheit, Forwarding, Eventkorrelation | OpenAPI-Dokumentation aus bestätigten Verträgen |
| Migration, Queue-Cutover, Restore | UI-Feinschliff und reproduzierbare Regressionstests |

Die Umsetzung erfolgt nacheinander in überprüfbaren Paketen. Ein großes „Relaykit umbenennen und Stalwart anschließen“-Commit würde die kritischen Grenzen verdecken. Ein zusätzlicher SDK-Bau, generischer Provider-Marktplatz oder Kampagnensystem ist nicht erforderlich.
