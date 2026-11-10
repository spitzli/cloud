# Webdock Managed PostgreSQL Implementation Plan

> **For agentic workers:** Use `superpowers:executing-plans` for implementation task-by-task; use subagents only if the user explicitly selects delegation. Checkboxes track deliverables. This is the requested plan, not authorization to deploy, purchase, migrate existing customer data, commit or publish.

**Goal:** Shared-Datenbanken und eigene PostgreSQL-Instanzen auf dem vorhandenen Drei-VPS-k3s-Cluster als bezahlte, dauerhaft aktive Angebote ab erster Freigabe durch Kunden selbst buchen und verwalten lassen. Provisionierung nur nach expliziter Bestellung oder entsprechend angenommenem Produktangebot.

**Architecture:** CloudNativePG verwaltet PostgreSQL-Installationen mit eigenem Namespace/Volume: entweder einen Plattform-Shared-Pool mit getrennten logischen Kundendatenbanken oder eine eigene Instanz für eine Bestellung. Kundenressource, Bestellung und physische Installation sind getrennte Modelle. Bestehende Auth-/Hosting-API, Agent, Operationsjournal, Angebote und Quoten werden erweitert. SQL-Verbindungen gehen direkt zur Datenbank bzw. PgBouncer; Verwaltung bleibt auf Vercel, physische Backups liegen extern je Installation.

**Tech Stack:** Bestehendes Node 24 / Next.js 16.3.8 / TypeScript / Zod / PostgreSQL-Control-Plane / Python-Agent / k3s / cert-manager; zusätzlich CloudNativePG und Barman Cloud Plugin, PostgreSQL und bei Bedarf PgBouncer. Kein neuer Queue-Service, kein eigener Kubernetes-Operator, kein pauschaler Longhorn-Ausbau.

**Spec:** [Produkt- und Architekturentwurf](../specs/2026-10-12-managed-postgresql-design.md).

## Aktuelle Ausführungsentscheidung des Eigentümers — 2026-10-12

Vollständige Umsetzung einschließlich nativer Webdock-Operatorverwaltung ist beauftragt. **Backups sind vorerst ausdrücklich zurückgestellt** (Folgenachricht des Eigentümers). Keine Backupdienste, S3-Buckets, Barman-Jobs, WAL-Archive oder Backupbestellungen aktivieren. Aufgabe 6, PITR, backupbasierter Clone/Restore und deren Abnahme bleiben TODO. API/Studio zeigen „not configured“ statt Erfolg oder Recovery-Fenster; entsprechende Aktionen bleiben nicht verfügbar. Angebote und Bestellbestätigungen müssen ausdrücklich ohne Backup und ohne Wiederherstellungszeitraum ausgewiesen werden. Die technische Abnahme bleibt erforderlich; die ausdrückliche Zurückstellung der Backups blockiert die übrige Umsetzung nicht.

- [ ] TODO nach ausdrücklicher Wiederaufnahme: EU-Backupziel auswählen, Provenance/Sidecar-Pins prüfen, Budget reservieren, WAL/Base-Backups einrichten, PITR/Restore und Alarmzustellung nachweisen.

## Global Constraints

- Planungsauftrag; Infrastruktur bleibt unverändert. Alle Tarife und Kapazitäten sind Vorschläge bis zur Pilotabnahme.
- Beide Betriebsarten sind bezahlt und zum ersten Kundenlaunch selbst buchbar. Kein Operator-pro-Kunde-Provisionierungsschritt, kein automatisch erzeugtes Konto-/Projekt-DB-Add-on und kein Free-Tarif.
- Bestehende drei Contabo-EU-VPS; keine neuen Bestellungen und kein Hetzner-Ausbau.
- Vorhandene uncommittete Änderungen, alte Backups und produktive App-Volumes erhalten. Ausführung später in isoliertem Checkout auf geprüftem aktuellem Stand.
- Node 24; vor Next.js-Änderungen lokale AGENTS.md und relevante installierte Framework-Dokumentation lesen.
- Plattform-IDs bleiben Snowflake-Dezimalstrings, Byte-/CPU-Mengen begrenzte Integer. GiB und GB niemals still gleichsetzen.
- EU-Ausführung und bestehende Frankfurt-Konfiguration beibehalten; kein pauschales EU-only-Versprechen für unveränderte globale Dienste.
- Additive, explizite Migrationen; kein Schema-Push aus HTTP-Requests. Keine Produktionsdatenbank für Tests.
- Verträge, Agent und UI müssen alte App-, BYOK-, Mail- und Storage-Operationen weiter unterstützen.
- Keine Kundenschreibrechte auf Kubernetes/CNPG-Ressourcen, Shell, Superuser-SQL oder ungeprüfte Extensions.
- Zerstörerische Aktionen und Purge nur über die bestehenden operatorgeschützten Webdock-Verfahren mit frischer Vorschau und Bestätigung.
- Englische UI-Quelltexte und bestehende deutsche i18n-Kataloge verwenden.
- Backups sind eine neue geplante Abnahmevoraussetzung für dieses DB-Produkt; zuvor vertagte Cluster-Backups werden damit nicht heimlich aktiviert.

## Review Focus

1. Buchung doppelt abgeschickt oder Agentenantwort nach Erstellung verloren: keine doppelte DB, Credential-Revision, Rechnung oder Reservation; ohne angenommene Bestellung kein Create. Aufgaben 3/4/7/9.
2. Shared-SQL-Zugriff über fremden DB-Namen, PUBLIC, Rollenwechsel, Pooler oder Restore: keine fremden Daten/Credentials, keine Kundenaktionen am gesamten Pool. Aufgaben 3/4/5/6/8/9.
3. Clustersumme passt, Volume-Knoten/Pool ist voll oder Kapazität bereits Apps zugesagt: physisches Budget einmal je Installation, logische Kontingente separat, voller Pool-Restore berücksichtigt. Aufgaben 1/2/3/9.
4. CNPG-Ausnahmen, abgelaufene Lease oder verlorener Mount: keine zusätzlichen App-Rechte, keine leere Ersatz-DB, kein zweiter freigegebener Writer. Aufgaben 2/4/8/9.
5. Zertifikat, Archivierung, Passwortrotation oder Kunden-Restore scheitert teilweise: letzter gültiger Zustand bleibt sichtbar; keine falsche Ready-/Backup-Erfolgsmeldung und keine Veränderung fremder Shared-DBs. Aufgaben 5/6/7/9.

## Vorhandene Integrationspunkte

| Bereich | Bestehender Pfad | Anpassung |
| --- | --- | --- |
| Verträge | `packages/hosting-contracts/src/index.ts` | Separaten DB-Vertrag re-exportieren und Commands ergänzen |
| Geschäftslogik | `apps/auth/src/lib/hosting/service.ts` | DB-Kommandos mit vorhandener Autorisierung dispatchen |
| Schema | `apps/auth/src/lib/hosting/schema.ts` | Additives DB-Schema anbinden |
| Quoten | `apps/auth/src/lib/hosting/reservations.ts`, `allowances.ts` | Installation/Pooler/Restore im physischen Ledger, Shared-Kontingente separat |
| Operationen | `apps/auth/src/lib/hosting/operations.ts`, `agent-api.ts` | Typisierte DB-Pakete, Fähigkeiten, Status und Beobachtungsnachweise |
| Agent | `apps/hosting-agent/daemon.py`, `executor.py`, `observer.py` | Eigenen CNPG-Ausführer einhängen; alte Ausführung erhalten |
| Speicher | `apps/hosting-agent/storage.py`, `storage_client.py`, `storage_service.py` | Eigenen DB-Pool-/Volume-Adapter mit erhaltener v1-App-Semantik |
| Studio | `apps/admin/src/components/hosting/`, `apps/admin/src/lib/hosting-client.ts`, `hosting-api.ts`, `hosting-actions.ts` | DB-Liste, Details und Aktionen ergänzen |
| Kunden-/Projektseiten | `apps/admin/src/app/(console)/(portal)/tenants/[customerID]/hosting/page.tsx`, `hosting/projects/[projectID]/page.tsx` | DB-Angebot und Ressourcen zuordnen |
| MCP | `apps/admin/src/lib/hosting-mcp.ts` | Nach funktionierendem REST dieselben geprüften Commands anbieten |
| Tarife/Buchung | `apps/auth/src/app/admin/plans/`, `apps/auth/src/app/tenants/[customerID]/usage/`, `apps/auth/src/app/offers/` | Kaufbare DB-Angebote, kundenbestätigte Aufträge und Tarifwechsel; bestehende manuelle Rechnungen, kein neuer Zahlungsanbieter |

## API- und Zustandsvertrag

Neu: `packages/hosting-contracts/src/databases.ts`. `resourceID` und Idempotenzvalidatoren aus bestehenden Verträgen wiederverwenden. Die folgenden Typen definieren den geplanten öffentlichen Vertrag; kein öffentlicher frei bearbeitbarer CNPG-Spec.

```ts
export type DatabaseKind = 'shared' | 'dedicated';

export type DatabaseState =
  | 'provisioning' | 'ready' | 'pausing' | 'paused' | 'resuming'
  | 'updating' | 'restoring' | 'degraded' | 'deleting'
  | 'retained' | 'purged' | 'needs-reconciliation';

export type DatabaseCreate = {
  action: 'databases.create';
  projectID: string;
  name: string;
  planRevisionID: string;
  orderID: string; // accepted immutable order with matching project/kind/plan
  kind: DatabaseKind;
  idempotencyKey: string;
};

export type DatabaseAction = {
  action: 'databases.pause' | 'databases.resume' | 'databases.restart'
    | 'databases.backup' | 'databases.rotateCredentials';
  databaseID: string;
  revision: number;
  idempotencyKey: string;
};

export type DatabaseCopy = {
  action: 'databases.restore' | 'databases.clone';
  sourceDatabaseID: string;
  targetProjectID: string;
  name: string;
  restorePointID: string;
  targetOrderID: string; // authorizes the target resource and its cost
  revision: number;
  idempotencyKey: string;
};

export type DatabaseResize = {
  action: 'databases.resize';
  databaseID: string;
  revision: number;
  targetPlanRevisionID: string;
  targetOrderID: string;
  idempotencyKey: string;
};

export type DatabaseView = {
  id: string;
  projectID: string;
  name: string;
  revision: number;
  observedRevision: number;
  state: DatabaseState;
  planRevisionID: string;
  kind: DatabaseKind;
  postgresMajor: number;
  memoryBytes: number | null; // null for shared: no individual RAM promise
  cpuMillicores: number | null;
  volumeBytes: number | null; // dedicated filesystem; not per shared DB
  logicalStorageAllowanceBytes: number | null; // shared measured allowance
  storageEnforcement: 'measured-logical' | 'bounded-filesystem';
  databaseName: string;
  endpoint: { host: string; directPort: number; pooledPort: number | null } | null;
  backup: { lastSuccessAt: string | null; recoverableUntil: string | null;
    lastRestoreTestAt: string | null };
  operationID: string | null;
  observedAt: string | null;
};

export type DatabaseOrderAccept = {
  action: 'databases.acceptOrder';
  offerID: string; // tenant-bound immutable price, kind, terms and plan revision
  projectID: string;
  idempotencyKey: string;
};
```

`restorePointID` verweist auf einen serverseitig geprüften Backup-/PITR-Katalogeintrag, nicht auf frei eingegebene Object-Storage-URLs. Ein Shared-Kunde sieht nur seine logisch zulässigen Restorepunkte, keine fremden Pool-Ressourcen. Copy-Ziele gehören zum gleichen Kunden; Betreiberimpersonation erteilt keine zusätzlichen Kundenrechte. CPU/Storage/Pakete können Kunden nur innerhalb freigegebener Angebote wählen. Nicht angenommene Aufträge dürfen keine Ressource erzeugen; bereits verbrauchte Auftragsreferenzen dürfen keine zweite Ressource erzeugen; manuelle Rechnungsstellung bedeutet nicht, dass bereits vor Bereitstellung eine Zahlung vorliegen muss. Gültige Angebotsannahme ist die kommerzielle Autorisierung. Operator-only `reconcile`, `deletionPreview`, `delete`, `purgePreview`, `purge` bleiben erhalten; Kunden können Vertragsbeendigung selbst beantragen und sehen deren Status. Keine Geheimnisse in `DatabaseView`, Operationsergebnissen, Logs oder MCP-Ausgaben. Abruf von Zugangsdaten ist ein eigener frischer autorisierter UI-Vorgang mit `no-store`; erfolgreiche Offenlegung wird ohne Secret protokolliert.

Installation/Backend-Modell nur für Betreiber: Backend-ID, `shared-pool|dedicated`, CNPG-UID, Namespace, Knoten/Pool-/PV-IDs, physische Reservation, Zustand und Backend-Revision. Kunden-DB referenziert dieses Backend. Shared-Backend-Änderungen werden zusätzlich zur Projektoperation backendweit serialisiert; konkurrierende Kunden-Updates dürfen keine Rollenliste oder Kontingente überschreiben. Eigene Instanzen können nach ausdrücklicher Upgradebestellung Ziel eines logischen Datenumzugs sein. Ein Moduswechsel ist keine einfache Änderung des `kind`-Felds.

Kundenfähigkeit nach Betriebsart: Shared `pause/resume` sperrt/entsperrt ausschließlich eigene DB-Zugänge und beendet nur deren Sessions. Shared `restart` wird als nicht unterstützt abgelehnt, nie zum Pool-Neustart übersetzt. Shared `resize` ändert ausschließlich gebuchtes logisches Kontingent, solange Poolreserve reicht. Ganze Pool-Upgrades, CNPG-Hibernation, physische Backups und globale Parameter bleiben Betreiber-/Automationsaufgaben. Eigene Instanzen bieten die entsprechenden geprüften Instanzaktionen. Kunden-Backupaktionen dürfen keine frei zugänglichen physischen Shared-Backups liefern.

DB-Status ist getrennt vom bestehenden Operationsstatus `queued/running/succeeded/failed/needs-reconciliation`. Lange Backups/Restores blockieren keinen Vercel-Request und keine minutenlang offene Agentenschleife. Ein kurzes typisiertes Kommando erzeugt eine eindeutig markierte CNPG-Ressource; spätere begrenzte Beobachtungen bestätigen den terminalen Zustand. Eine abgelaufene Lease bedeutet unklaren Ausführungsstand, nicht Nichtexistenz. Keine automatische Wiederholung unsicherer Mutationen.

## Aufgabe 1 — Bestand und Größenmodell prüfen

**Ausführungsstand 2026-10-12:** begonnen. Wiederholbares Read-only-Inventar, Live-Nachweise, Größenrechnung und Image-Pins liegen in [der Baseline](../../verification/managed-postgresql-baseline.md). Aufgabe 1 bleibt offen: frisches Control-Plane-Reservierungsinventar, vollständige Herkunftsprüfung und gemessener Overhead fehlen; der isolierte CNPG-/Storage-Pilot wurde noch nicht ausgeführt.

**Dateien:** neu `docs/verification/managed-postgresql-baseline.md`, `infra/postgres/versions.env`, `infra/postgres/README.md`.

**Ergebnis:** belegte Versionen, nutzbare Knoten, reale Reservationen und ein geprüftes Pilotbudget.

- [ ] Dokumente mit Live-Inventar vergleichen: k3s/Kubernetes, CNI/NetworkPolicy, etcd-Mitglieder, cert-manager, bestehende Webdock-Identitäten und Agentversionen.
- [ ] Je Knoten Allocatable, Requests/Limits bestehender Pods, Hostbelegung, vorhandene Poolreservierungen, freie Disk und Mounts erfassen. Keine Dateninhalte oder Secrets exportieren.
- [ ] Paketanforderungen in Byte/Millicores rechnen. Shared-Pool zunächst 2 GiB/1 CPU/30 GiB mit maximal acht kleinen 1-GiB-Kontingenten als Messprofil; zusätzlich maximal zwei eigene Starter. Physische App-/DB-Admission gemeinsam, Shared-CPU/RAM nicht je Kunde erneut zählen. Bestehenden 20-Prozent-Guard genau einmal anwenden und Platz für einen vollen Pool-Restore einrechnen.
- [ ] Unterstützte CNPG-/Plugin-/Postgres-/Pooler-Versionen wählen, Signaturen prüfen und Digests festhalten. Nur freigegebene Images auf die benötigten Knoten bringen.
- [ ] Mit isolierten Testdateien begrenzte Storage-Messung und synthetisches `pgbench` planen; keine Vollplatten- oder Lasttests auf Kundendaten. Ergebnisse entscheiden über Pilotplatzierung und Pakete.

**Abnahme:** keine Kapazitätszahl wird aus der alten Momentaufnahme als frei übernommen; zwei gleichzeitige Pilot-Provisionierungen können nicht dieselbe letzte Kapazität verkaufen.

## Aufgabe 2 — CNPG und begrenzten Speicher qualifizieren

**Dateien:** neu `infra/postgres/operator.yaml`, `boundary.yaml`, `storage.md`, `profiles.yaml`; neu `apps/hosting-agent/database_storage.py`, `test_database_storage.py`; gezielte Integration in die bestehenden Storage-Module.

- [ ] In isolierter Kubernetes-Testumgebung CNPG und Plugin installieren, gezielte Installations-Namespace-/RBAC-/Admission-Profile einrichten. Einen Shared-Pool mit zwei synthetischen Kunden sowie eine eigene Instanz qualifizieren.
- [ ] Einen echten PostgreSQL-Cluster auf einem vorallokierten Test-Volume betreiben. CNPG-PVC-Template an das vom Helper erzeugte PV binden; Eigentümer, Dateirechte und UID-Prüfung festlegen. CNPG-Pod/PVC-Verwaltung nicht durch einen zweiten Pod-Controller ersetzen.
- [ ] Storage-Adapter akzeptiert ausschließlich DB-ID, Instanz-ID, geprüfte Revision, Knoten-/Pool-ID und begrenzte Bytezahl; keine Kundenpfade oder Shellstrings.
- [ ] Testfälle: Volume voll, Journal fehlt, Mount fehlt, falscher Eigentümer, Hostreserve unterschritten, Helper-Neustart, Pod-Neustart. Fehlende Daten müssen blockieren statt neu initialisiert werden.
- [ ] Zuerst vorhandenen Knoten-02-Poolpfad mit separatem DB-Pool qualifizieren. Für spätere Knoten separate Helper-/Pool-Identitäten und authentisierte Beobachtung einrichten; Rootoperationen bleiben lokal eingeschränkt. Administrative Bereitstellung darf initial manuell erfolgen.
- [ ] DB-spezifische API-/Dateisystem-Ausnahmen dürfen auf normale Kunden-App-Namespaces nicht wirken. Admission-Test mit unzulässigem Pod und fremder PVC muss weiterhin scheitern.

**Abnahme:** synthetische Tabellen überleben Pod- und Helper-Neustart; ENOSPC trifft nur das DB-Volume; keine fremden Secrets/PVs erreichbar. Pilot weiter auf einem Knoten erlaubt, kommerzielle Mehrknoten-Kapazität erst nach dessen eigener Prüfung.

## Aufgabe 3 — DB-Modell, Berechtigungen und Quoten

**Dateien:** neu `packages/hosting-contracts/src/databases.ts`, `databases.test.ts`; neu `apps/auth/src/lib/hosting/databases.ts`, `database-schema.ts`, `database-orders.ts`; neu `apps/auth/scripts/migrate-hosting-databases.ts`, `apps/auth/tests/hosting-databases.test.ts`, `hosting-database-orders.test.ts`; vorhandene Schema-/Service-/Reservierungs-/Tarifdateien erweitern.

- [ ] Strikte Commands und Ansichten nach obigem Vertrag definieren. Revisionskonflikte und wiederverwendete Idempotenzschlüssel mit anderem Inhalt ablehnen.
- [ ] Additive Tabellen für Bestellung, verkaufte logische DB-Ressource, physisches Backend/Shared-Pool, Platzierung/Volumes, Backups/Restore-Katalog und Endpunkt-/Portreservierung anlegen. Kunde/Projekt, Modus, Datenbankname, CNPG-UID, Planrevision, Credential-Revision und beobachteter Zustand sind explizit. SQL-Migration bleibt separat ausführbar.
- [ ] Kundenadmin akzeptiert ein serverseitig erzeugtes, tenantgebundenes Angebot mit unveränderlichem Preis/Modus/Leistung; Annahme, Kapazitätsprüfung, Auftrag und Provisionierung transaktional/idempotent verbinden. Registrierung und Projektanlage tun das nie automatisch. Bereits angenommener Produktauftrag mit expliziter DB-Leistung kann denselben Pfad nutzen.
- [ ] Shared: Backendkapazität und Slots/logische Kontingente unter gemeinsamer Backend-Sperre reservieren. Dedicated: neues Backend plus physischen Bedarf reservieren. Laufende Installation samt Pooler/Sidecars nur einmal physisch zählen. Keine doppelte Reservation bei Shared-Retry, vollständige Zusatzkapazität bei Pool-Restore. Abrechnungsbeginn genau einmal aus bestätigtem Ready ableiten; Fehlschlag erzeugt keine aktive Leistung.
- [ ] Berechtigungen bei jeder Mutation frisch prüfen, inklusive Projektzuordnung, archiviertem Kunden, Tenant-Preview und Tarifrevision. Geheimnisse mit bestehender Verschlüsselung und Zweckbindung `database:<ID>:<credentialRevision>` speichern.
- [ ] Gezielte Tests schreiben und zuerst scheitern lassen: fremder Kunde; kein angenommener Auftrag; manipuliertes Shared-Angebot als Dedicated; Registrierung ohne DB; doppelte Angebotsannahme; Preview-Mutation; race auf letzten Poolslot/letztes RAM; richtige Clustersumme/falscher Knoten; paused DB ohne doppelte RAM-Verkäufe; retained Daten; Copy ohne ganzen Recovery-Pool-Platz; identischer Retry; keine Rechnung vor Ready.
- [ ] Commands implementieren, Tests grün machen, additive Migration auf Wegwerf-DB zweimal ausführen und Abwärtskompatibilität prüfen.

**Abnahme:** kein bezahlter Tarif wird allein durch Anlegen eines Templates aktiven Kunden zugewiesen; keine Ressource wird ohne nachgewiesenen Abbau freigegeben.

## Aufgabe 4 — Agent-Protokoll und CNPG-Lebenszyklus

**Dateien:** neu `apps/hosting-agent/databases.py`, `test_databases.py`; bestehende `daemon.py`, `observer.py`, `executor.py`, `operations.ts`, `agent-api.ts` erweitern.

- [ ] Neue Fähigkeit `databaseVersion: 1` melden. Alte Agenten dürfen DB-Aufträge weder claimen noch als erfolgreich quittieren.
- [ ] Installation ausschließlich aus geprüftem Profil rendern: Namespace, Eigentumslabels, Quota/Policies, Secrets, Cluster und Pooler. Shared-Bestellung erzeugt logische DB und Rollen im freigegebenen Pool; Dedicated-Bestellung erzeugt eigene Installation. CNPG-Database-/Rollenverwaltung der gepinnten Version verwenden; ACL-Konfiguration nur über festes Betreiberverfahren, keine frei übermittelten SQL-Kommandos.
- [ ] Im Shared-Pool DB-spezifische Login-/Migrations-/App-Rollen, PUBLIC-Entzug, erlaubte Datenbank-/Rollenpaare und Positivliste für Extensions einrichten. NOLOGIN-Owner bleibt Plattformkontrolle, keine CREATEROLE-/CREATEDB-/Superuser-Mitgliedschaften für Kunden. Neue Login-Rollen erst nach bestätigter Isolation freigeben; Kundennamen nicht in sichtbare Pool-Kataloge schreiben.
- [ ] Create, pause, resume, restart und Ressourcenänderung nach Betriebsart implementieren. Shared-Pause betrifft eigene Rollen/Sessions; Shared-Restart ablehnen. Hibernation nur eigene Instanz, ohne Volume-/Reservation-Löschung. Resume bleibt auf vorhandene Pool-/PV-Identität gebunden. SQL-DB-Erstellung ist nicht Teil einer verteilten ACID-Transaktion; Status/Recovery müssen Teilfertigstellung nachweisen, ohne fremde Objekte zu löschen.
- [ ] Erfolgsnachweis an DB-ID, CNPG-UID, gewünschte Revision, tatsächliche Ressourcen und Readiness binden. CNPG-Reconciliation mit Statusabfrage beobachten statt `kubectl apply` allein als fertig zu bewerten.
- [ ] Leases, Generationen und Idempotenz aus dem bestehenden Protokoll erhalten. Unterbrochene Mutationen werden über vorhandene markierte Kubernetes-Objekte abgeglichen. Passwortgenerierung nicht bei Retry wiederholen.
- [ ] Tests: verlorene Antwort nach Create; Lease läuft aus; fremdes Kubernetes-Objekt; altes Agentenpaket; CNPG startet nicht; Pause während Backup; Resume; Agent aus. Zusätzlich echte SQL-Tests Kunde A → DB B/postgres/template1; SET ROLE/Privileggewinn; falsche Pooler-Zuordnung; starke Konkurrenzlast; Shared-Pause/Restart-Anfrage darf B nicht unterbrechen; zwei gleichzeitige Rollenänderungen dürfen sich nicht überschreiben.

**Abnahme:** genau eine DB für einen Create-Request; Control-Plane-/Agenten-Ausfall stoppt keine gesunde laufende PostgreSQL-Instanz; unsicherer Zustand bleibt sichtbar.

## Aufgabe 5 — TLS-Zugänge und Vercel-Kompatibilität

**Dateien:** neu `infra/postgres/endpoints.yaml`, `certificates.yaml`; neu `apps/auth/src/lib/hosting/database-endpoints.ts`, `apps/auth/tests/hosting-database-endpoints.test.ts`; Agentprofil aus Aufgabe 4 ergänzen.

- [ ] Vorhandene NodePorts/öffentliche Ports inventarisieren, exklusiven Bereich in der DB-Endpunkttabelle reservieren. Portkollisionen durch DB-Constraint und Kubernetes-Prüfung verhindern.
- [ ] Pro Installation stabilen Hostnamen sowie direkten/Pooler-Port erzeugen. Shared-Kunden teilen Endpunkt, jedoch nicht Datenbankname oder Rolle. Intern ClusterIP verwenden. Öffentliche Endpunkte gehen an einen fest dokumentierten Zugangsknoten; kein automatisches DNS-HA behaupten.
- [ ] Native PostgreSQL-/PgBouncer-TLS-Zertifikate über cert-manager mit passendem Hostnamen ausstellen. Bestehendes HTTP01-Verfahren und dessen Namespace-Grenzen prüfen. Plaintext-Verbindungen sperren.
- [ ] Plattform-NOLOGIN-Owner, Kundenschema-Migrationsrolle und App-Rolle ohne Superuser einrichten. Pooler pro DB/Rolle begrenzen; direkte Migrationsverbindungen mit eigenen Limits vorsehen. Rotation muss Secret, PostgreSQL und Pooler zusammenführen, ohne andere Rollen neu zu starten/rotieren; Abbruch hält letzte bestätigte Credential-Revision fest.
- [ ] Aus Vercel und lokal mit `psql`, `pg` und Prisma prüfen: richtiger Host funktioniert; falscher Host, abgelaufenes Zertifikat und falsches Passwort scheitern; Limits greifen; Verbindung kann nach Pod-Neustart neu aufgebaut werden.
- [ ] Neon-HTTP-Treiber-Migration dokumentieren. Kundenzugang bleibt normaler PostgreSQL-TCP-Zugang; Webdock ist kein SQL-Proxy.

**Abnahme:** funktionierender Anwendungspfad einschließlich TLS-Prüfung und Pooling, nicht nur offener Port. Keine DB-Credentials in generischen API-/MCP-Antworten oder Logs.

## Aufgabe 6 — Externe Backups und belegter Restore

**Dateien:** neu `infra/postgres/backup.yaml`, `monitoring.yaml`; neu `apps/auth/src/lib/hosting/database-backups.ts`, `apps/auth/tests/hosting-database-backups.test.ts`; Agentmodul ergänzen; neu `docs/operations/managed-postgresql-restore.md`.

- [ ] Bestehenden geeigneten externen EU-S3-Speicher prüfen oder ein konkretes Angebot zur späteren Buchung vorlegen. Bucket/Präfix/Retention/KMS-Kosten und Plugin-Kompatibilität dokumentieren. Keine automatische Bestellung.
- [ ] Barman Cloud Plugin je Installation mit täglichem Base-Backup und sieben Tagen Recovery-Fenster konfigurieren; laufendes WAL-Archiv, begrenzte Jobressourcen und separate Wiederherstellungsberechtigungen. Shared-Backups enthalten alle Pool-Kunden; kein Kunden-Download oder direkter Restore-Zugriff darauf.
- [ ] Archivierung und Restore-Katalog anhand tatsächlicher CNPG-/Plugin-Zustände erfassen. Ein fehlgeschlagenes Backup überschreibt den letzten Erfolg nicht; ein erfolgreicher Job allein bedeutet nicht, dass jeder Zeitpunkt wiederherstellbar ist.
- [ ] Dedicated-RestorePoint auf neue reservierte Instanz abbilden. Shared-RestorePoint: privaten ganzen Pool wiederherstellen, ausschließlich die betroffene DB mit `pg_dump` exportieren und unter neuen Zielrollen ohne globale Owner/ACLs importieren; keine Wiederherstellung kundenkontrollierter Dump-Inhalte als Superuser. Recovery-Pool bleibt für sämtliche Kunden unerreichbar. Neue Kundenziel-DB bis Abnahme nicht öffentlich, neue Credentials, keine ausgehenden Produktionsintegrationen. Quelle und fremde DBs unverändert behalten.
- [ ] Zwei Shared-Kunden schreiben unterschiedliche Testdaten. Daten von A löschen, A auf früheren Zeitpunkt wiederherstellen, während B neue Daten schreibt. B muss unverändert aktuell und unerreichbar für A bleiben. Dedicated-PITR zusätzlich prüfen. Knotenverlust durch unzugängliches Quellvolume simulieren; gesamten Shared-Pool aus externem Backup auf anderem qualifiziertem Knoten wiederherstellen.
- [ ] Backupziel-Ausfall und WAL-Rückstand auslösen; Warnung, tatsächlichen Recovery-Stand und Disk-Wachstum prüfen. Kein automatisches Löschen benötigter WALs. Restorezeit und Verlustfenster messen.
- [ ] Wöchentlichen rotierenden Restore-Test und echte Alarmzustellung einrichten. Fünf-Minuten-Archivierungsziel und 60-Minuten-Ziel für kleine eigene Instanz erst nach Messung dokumentieren. Shared-Kunden-Recovery am gesamten Pool messen und eigenes Zeitbudget ausweisen; noch kein Kunden-SLA.

**Abnahme:** getesteter Restore ohne Zugriff auf das Quellvolume, richtige Daten und nachvollziehbare Recovery-Zeiten. Ohne diese Abnahme bleibt das Angebot ein interner Pilot.

## Aufgabe 7 — Studio, Angebote und Betriebsansicht

**Dateien:** neu `apps/admin/src/components/hosting/databases.tsx`, `database-details.tsx`; neu `apps/admin/src/app/(console)/(portal)/hosting/databases/[databaseID]/page.tsx`; bestehende Hosting-Seiten, API-Adapter, MCP, i18n, Tarife/Usage erweitern.

- [ ] Kundenflow ab v1: Projekt → Datenbank hinzufügen → Shared/eigene Instanz wählen → unveränderlichen Preis und Grenzen prüfen → kostenpflichtig bestellen → Provisionierungsfortschritt → Zugangsdaten. Ohne Auswahl bleibt das Projekt ohne DB. Keine manuelle Operatorfreigabe je Standardbestellung.
- [ ] Liste und Details zeigen Modus/Paket, gewünschten/beobachteten Status, Nutzung, letzte Messung, Endpunkte, Backupzustand und laufenden Auftrag. Bei Shared CPU/RAM als gemeinsam genutzt sowie Speicher als gemessenes Kontingent anzeigen, keine garantierten Fantasiewerte. Stale/unknown nicht als null Verbrauch oder gesund anzeigen.
- [ ] Kundenadmin verwaltet eigene freigegebene Aktionen, Betreiber unterstützt bei Wiederherstellung/Migration. Bestandsangebote bleiben unverändert. Optionale DB in einem Hostingangebot muss vor der Annahme ausdrücklich ausgewählt/enthalten sein.
- [ ] Zugangsdaten getrennt geschützt offenlegen; Verbindungsbeispiele enthalten TLS, direkten/Pooler-Unterschied und keine echten Secrets in Screenshots/Testartefakten.
- [ ] Pause erläutert Unterbrechung und unveränderten Paketpreis; bei Shared heißt die Aktion Zugang pausieren, nicht Server stoppen. Nur eigene Instanz bietet Restart/CPU-/RAM-Resize. Upgrade Shared → eigene Instanz zeigt neuen Preis und erforderliches Migrationsfenster. Restore zeigt neue Kunden-DB und deren Preis/Bedarf, nie fremde Recovery-Pool-Inhalte.
- [ ] Vorhandene Tarifvorlagen/Annahmehistorie um kundeninitiierte DB-Bestellannahme erweitern. Shared 5,90 EUR, eigene Starter-Instanz 14,90 EUR netto als Entwürfe; Standardgröße später 24,90 EUR. Keine automatische Tarifänderung, kein Free-Tarif und keine neue Zahlungsintegration. Rechnungsbeginn an erfolgreiche Bereitstellung binden.
- [ ] Gleiche Autorisierung über Studio, REST und spätere MCP-Werkzeuge prüfen. Browser-Abnahme mit Operator, zwei getrennten Kunden und Read-only-Preview.

**Abnahme:** Kunde bucht beide Varianten vollständig ohne Betreiberhilfe; ohne Bestellung entstehen weder Ressourcen noch Rechnung. Vollständiger Flow Panel → Auth → Agent → CNPG → beobachteter Zustand. Query-Latenz/Backup-Metriken nur mit implementierter Messquelle anzeigen.

## Aufgabe 8 — Wartung, Copy, Löschung und Migration

**Dateien:** neu `apps/auth/src/lib/hosting/database-maintenance.ts`, `apps/auth/tests/hosting-database-maintenance.test.ts`; Agent ergänzen; neu `docs/operations/managed-postgresql-maintenance.md` und `managed-postgresql-migration.md`.

- [ ] Clone nutzt je nach Modus den sicheren Restore-/logischen Importpfad, eigene Kunden-DB-ID/Credentials und korrekte logische oder physische Reservation. Keine automatische Anwendungskopie und keine still aktivierten Jobs/Webhooks.
- [ ] CPU/RAM-Upgrade nur bei eigener Instanz nach Platzierungs-/Tarifprüfung. Shared-Kontingent wächst nur innerhalb freigegebener Poolkapazität. Kundenbestelltes Shared → Dedicated durch logischen Testimport, finales Schreibfenster, neue Endpunkte und Integritätsprüfung; kein Modusflag als Migration. Physisches Storage-Upgrade durch neues größeres Volume/Datenumzug, keine behauptete Online-Vergrößerung fester Filesystems.
- [ ] Patch-/Major-Upgrade zuerst an einer Kopie einschließlich Extension-/Anwendungstest. Wartungsfenster, Sicherung und Downtime benennen. Kein Image-Downgrade auf bereits major-upgegradeten Daten.
- [ ] Kundenkündigung ist selfservice; destruktive Ausführung bleibt im geprüften Operatorprozess. Dedicated-Löschung entfernt eigene Endpunkte/Laufzeit und behält Recoverydaten bis zur Frist. Shared-Löschung sperrt ausschließlich eigene Zugänge, niemals Pool/Ports/Namespace/Volume löschen. Nach Frist nur die eigene logische DB und ihre unreferenzierten Rollen/Secrets entfernen. Physische Poolkapazität bleibt reserviert; logische Freigabe bedeutet keine sofortige physische Backup-Löschung. Geteilte Backups laufen über die offengelegte begrenzte Retention aus. Vorschlag sieben Tage nach Vertragsende, mit genau dokumentierter Backup-Ablauffrist; keine sofortige vollständige Löschzusage.
- [ ] Bestehende Neon-DB zuerst inventarisieren: Größe, Version, Extensions, Rollen, Treiber, Pooling, Jobs. Testimport; Tabellen-/Zeilen-/Integritätsprüfungen; finales Schreibfenster stoppen, finalen Datenstand übertragen und Verbindungsdaten umstellen. Kein pauschales Copy-Backup als Zero-Downtime-Migration verkaufen.
- [ ] Quelle zunächst unverändert aufbewahren. Nach Zielschreibzugriffen braucht Rückmigration einen Datenabgleich; kein blinder Rückwechsel auf alte Daten. Auth/Studio-DBs und produktive Kundendaten nicht mit dem Pilot vermischen.

**Abnahme:** Clone und Shared → Dedicated enthalten nur die eigene DB; Restore/Resize lassen genau einen freigegebenen Writer je betroffener DB zu; Shared-Pause/Kündigung/Delete/Purge lassen andere Pool-Kunden und deren Rollen/Backups erreichbar und unverändert.

## Aufgabe 9 — Gesamtprüfung und Pilotfreigabe

**Dateien:** neu `docs/verification/managed-postgresql-pilot.md`; Tests aus Aufgaben 2–8 vervollständigen.

- [ ] Gegen disposable lokale PostgreSQL-Testdatenbank ausführen: `npm run test:auth`; Agenttests: `python3 -m unittest discover -s apps/hosting-agent -v`; Vertragsprüfung gezielt mit dem bestehenden TS-Testaufbau. Neues Testmodul in den normalen Vertragstestlauf aufnehmen.
- [ ] `npm run check -w @spitzli/auth`, relevante Lints und `npm run build:auth` / `npm run build:admin` mit vorgesehenen Test-/Build-Umgebungen ausführen. Bestehende Fehlschläge von neuen Regressionen trennen.
- [ ] Auf isoliertem Cluster vollständige Buchungs-, Tenant-, SQL-Isolations-, Retry-, Volume-, TLS-, Backup-/Restore- und Upgrade-Tests ausführen. Shared-Nachbarlast, verändertes `work_mem`, direkte Verbindungen am Pooler vorbei und große Transaktionen testen; Messkontingente nicht als harte SQL-Quota abnehmen. Ausfall-/Volllastprüfungen auf Produktionsknoten nur im kontrollierten Fenster.
- [ ] Shared mit mindestens zwei synthetischen Kunden sowie eine nichtkritische eigene Instanz pilotieren. Mindestens 24 Stunden mit Konkurrenzlast, Backup und vollem Pool-Restore; p50/p95/p99, CPU-Throttling, Speicher, Disk-Latenz und WAL-Wachstum erfassen. Beide Kundenbuchungen mit echten Testidentitäten vollständig durchlaufen.
- [ ] Vor weiterer Freigabe dokumentieren: Freshness, Placement-Fit, tatsächlich freier Storage, Pooler-/Operator-/Backupverbrauch, Restore-Reserve und unveränderte bestehende App-Gesundheit.
- [ ] Nach internem Test beide Varianten gleichzeitig als Kunden-Selfservice freigeben, vorläufig maximal acht kleine Shared-DBs in einem Pool und zwei eigene Starter, soweit gemessen tragfähig. Kein öffentliches operator-only-Zwischenprodukt. Preis/Vollkosten mit echten Rechnungen/Backupverbrauch rechnen; Pool-Durchsatz nicht aus RAM allein ableiten.

**Abnahme:** alle Erfolgskriterien im Entwurf belegt, verbleibende Starter-Ausfallgrenzen verständlich im Angebot. Keine HA-/SLA-/Null-Downtime-Aussage ohne die entsprechende Prüfung.

## Aufgabe 10 — Ausbau nach dem Pilot

Eigenständige Folgeprojekte, nicht Voraussetzung für den funktionierenden Starter:

- [ ] DB-Pools auf alle drei geeigneten Knoten ausweiten, wenn der anfängliche Pilot nur einen nutzt; identische Mount-/Quota-/Ownership-Prüfung je Knoten.
- [ ] Standard-Paket und zusätzliche freigegebene Extensions anhand realer Last qualifizieren.
- [ ] Zusätzliche Selbstbedienung für Clone/Restore nach erfolgreichem Betreiberverfahren aktivieren. Buchung/Create/Zugangsdaten/Grundverwaltung für beide Varianten sind bereits v1 und dürfen nicht hierhin verschoben werden.
- [ ] Bei Nachfrage PostgreSQL-Standbys auf anderen VPS hinzufügen. Replikations-/Quorum-/Fencing-Strategie, N+1-Ressourcen, Agenten- und Zugangsfailover separat planen und testen. CNPG-Replikation benötigt keinen vorgeschalteten Longhorn-Zwang.
- [ ] Automatische Dev/Test-Zeitpläne für Hibernation erwägen. Automatisches Wake-on-Connect nur mit eigenem Bedarf-/Kosten-/Latenznachweis; kein transparenter 500-ms-Start versprechen.
- [ ] Zusätzlichen VPS erst nach dokumentiertem Engpass kalkulieren. Bestandsdatenbanken werden umgezogen, nicht durch Addition entfernten RAMs vergrößert.

## Reihenfolge und Aufwand

Reihenfolge: 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9. Aufgaben 5/6 können teilweise vorbereitet werden, benötigen aber die funktionierende Grundinstanz. Aufgabe 10 ist eine Roadmap, keine heimliche Erweiterung von v1.

Grobe Aufwandsschätzung für eine erfahrene Person nach Scope-Erweiterung: Infrastruktur-/Storage-Pilot 2–4 Arbeitstage; beide Betriebsarten/Buchung/Agent/Quoten/SQL-Isolation 5–8; TLS/Backup/einzelkundenfähiger Shared-Restore 3–5; UI/Wartung/Gesamtabnahme 4–6. Insgesamt etwa 14–23 Arbeitstage zuzüglich Beobachtungszeit. Schätzung, kein Liefertermin; Admission-Regeln, CNPG-Kompatibilität und sichere Mehrkunden-Recovery können Zusatzarbeit verursachen. Nach Aufgabe 2 neu schätzen. Kein zeilenweiser Komplettcode vor Qualifizierung der Storage-/Versionskombination.

## Rücknahme und Übergabe

UI-/API-Rollback versteckt neue Funktionen, löscht aber keine DB-Ressourcen oder Backups. Alte Agenten claimen neue DB-Operationen nicht. CNPG/CRDs und Storagehelper niemals deinstallieren, solange sie DBs/PVs verwalten. Auth-Migrationen bleiben additiv und erhalten neue Datensätze. Schon entstandene Daten müssen nach einer zurückgenommenen Freigabe weiter betreibbar und wiederherstellbar bleiben.

Plan zur fachlichen Prüfung vorgelegt. Erster konkreter Ausführungsschritt ist Aufgabe 1 mit frischem Read-only-Inventar und anschließend der isolierte Storage-/CNPG-Pilot. Es wurden beim Planen keine Produktionsserver abgefragt, keine Pakete installiert, keine Dienste gestartet, keine Daten migriert und keine Bestellungen/Deployments vorgenommen.

## Umsetzungsstand — 12. Oktober 2026

- Native Bestellung, Operator-Katalog/Poolverwaltung, Agent/CNPG, TLS, SQL-Isolation, Nutzung und Abrechnungshistorie sind implementiert und lokal integriert geprüft.
- Wartung, Credential-Rotation, logische Migration, Aufbewahrung und gezielte Quell-/Instanzlöschung sind implementiert; Nachweise stehen in `docs/verification/managed-postgresql-*-pilot.json`.
- Studio-Browserabnahme mit Operator, zwei Kunden, Mandantentrennung und schreibgeschützter Kundenansicht erfolgreich. Auth 148, Studio 106, Contracts 17 und Infrastruktur 9 Tests bestanden; Agent 65 bestanden, 1 expliziter Docker-Mail-Test übersprungen. Beide Builds erfolgreich.
- Die 24-Stunden-Beobachtung der lokalen Shared-/Dedicated-Instanzen läuft seit 2026-10-12 19:15 UTC. Abschluss und dauerhafte Stabilität sind noch nicht nachgewiesen.
- Produktionsabnahme offen: Abgleich mit dem bereits ausgerollten App-Storage-Router/Operator-SSO, reale DNS-/Firewall-/CA-Konfiguration, externer Clientpfad einschließlich Vercel und Alarmzustellung. Noch kein Produktionsrollout.
- Backups/WAL/PITR/backupabhängige Wiederherstellung bleiben nach Eigentümerentscheidung TODO. Aufgabe 10 bleibt ein Folgeprojekt.

Betrieb, Installationsreihenfolge und Rücknahme: [Runbook](../../operations/managed-postgresql.md).
