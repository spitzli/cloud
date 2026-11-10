# Webdock Mail Relaykit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task; superpowers:subagent-driven-development is an alternative when parallel execution is explicitly selected. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Den vollständigen implementierten Relaykit-Funktionsumfang in Webdock Mail integrieren, mit Webdock-Mandanten, Plattform- und BYOK-Konten, ausdrücklich ohne Cloudflare-Integration.

**Architecture:** Die bestehenden Provider-/Tenantdienste in `apps/auth` werden um durable Nachrichten, eine eigene Kunden-API und Ereignisverarbeitung ergänzt. `apps/admin` stellt die integrierte Mail-Konsole über die bestehende SSO-Bridge bereit. PostgreSQL speichert Schedules, Idempotenz, Callback-Inbox und Webhook-Outbox; turboSMTP übernimmt die Zustellung.

**Tech Stack:** Node 24, Next.js 16.3.8 App Router, React 19.2.8, TypeScript, pg, Better Auth 1.7.7, bestehendes Webdock-i18n und npm-Workspaces; keine neue Auth-/ORM-/Queue-Plattform.

**Spec:** [Vollständiger Zielentwurf](../specs/2026-10-12-webdock-mail-relaykit-design.md).

**Status:** Vollständiger Plan zur Prüfung, keine Umsetzung gestartet. Benutzerkorrektur vom 12.10.2026 berücksichtigt: **keine CF-Implementation**. Alle Fortschrittskästchen bleiben offen, bis ihre tatsächliche Durchführung nachgewiesen ist.

## Global Constraints

- Node.js 24.x, Next.js 16.3.8, React 19.2.8 und die vorhandenen npm-Workspaces bleiben die Basis.
- Better Auth, Webdock-SSO, customer_id, organization_id und Snowflake-IDs bleiben maßgeblich; keine zweite Benutzerverwaltung.
- Geschäftslogik und Mail-Daten liegen in apps/auth; Studio in apps/admin verwendet die authentifizierte Studio-Bridge.
- turboSMTP bleibt der Provider; Versand verwendet kontogebundene Consumer-Key/Secret-Paare, niemals Master-Credentials als Kunden-Versandkonto.
- Neue Versandverbindungen verwenden EU-Endpunkte; bestehende explizite Global-Verbindungen werden als solche erhalten und angezeigt.
- Alle Mandantendaten werden über customer_id und connection_id abgegrenzt; fremde Ressourcen liefern 404.
- Eine unklare Provider-Annahme wird niemals automatisch erneut gesendet.
- Produktionsdaten, bestehende DNS-Einträge, Anbieter-Callbacks und fremde lokale Änderungen bleiben bis zum jeweils geplanten Cutover unangetastet.
- Authentifizierungs- und Einladungsmails bleiben auf dem vorhandenen sendAuthMail-Pfad; ein Mail-Produkt-Kill-Switch darf den Login nicht sperren.
- Vorschauumgebungen erhalten isolierte Datenbanken und Provider-Zugangsdaten; Tests senden keine echten E-Mails.
- Webdock-i18n, vorhandene UI-Komponenten, Tastaturbedienung sowie helle und dunkle Darstellung gelten für jede neue Seite.
- Es werden keine Neon-Auth-/Drizzle-Parallelimplementierung und kein zusätzlicher Queue-Dienst eingeführt.
- Cloudflare-Integration, Cloudflare-OAuth und automatische DNS-Schreibzugriffe sind ausdrücklich außerhalb dieses Plans.

## Review Focus

- Provider akzeptiert die Mail, danach geht die HTTP-Antwort oder der DB-Commit verloren: eine Message-ID bleibt prüfbar, kein automatischer Zweitversand. Tests T08/T11.
- Callback kommt vor der Send-Antwort, doppelt, verspätet oder mit fremder Account-/Empfängerzuordnung: nur die richtige lokale Projektion wird einmal fortgeschrieben. Tests T06/T11.
- Zwei unterschiedliche Consumer-Paare gehören zum selben BYOK-Account: kein zweiter Kunde darf dessen Daten/Callbacks beanspruchen. Tests T04/T19.
- Rechte, Domain, Connection oder Tenantzustand ändern sich zwischen Schedule-Annahme und Dispatch bzw. während eines Requests: der Dienst prüft aktuelle Grenzen, eingeschränkte Keys werden nicht erweitert. Tests T03/T05/T09/T17.
- Bodylimits, Bcc, HTML-Tracking, DNS-Rebinding und Retention greifen an unterschiedlichen Grenzen: keine Datenweitergabe oder Wiederholung durch Vorschau, Logs, Webhooks oder Cleanup. Tests T08/T12/T15/T18.

## 1. Ergebnis, Reihenfolge und Definition von „fertig“

Die Freigabe erfolgt nicht nach einer Anzahl implementierter Seiten. Jede Funktion F01–F21 aus der Spec hat Code, Tests, UI/API-Vertrag und gegebenenfalls Betriebsnachweis. Cloudflare-Automatisierung bleibt ausdrücklich ausgenommen.

| Etappe | Tasks | Prüffähiges Ergebnis |
| --- | --- | --- |
| A: Grundlage | T01–T05 | Testumgebung, Datenmodell, Keys, richtige Providerkonten und Domains |
| B: Versand | T06–T10 | Kanonische Events, Sperren, idempotenter Einzel-/Batch-/Scheduleversand |
| C: Rückmeldungen/API | T11–T14 | Zustellereignisse, signierte Kunden-Webhooks, Analytics, dokumentierte API |
| D: Konsole/Team | T15–T17 | Vollständiger Studio-Mailbereich und sicherer Teamzugriff |
| E: Betrieb/Cutover | T18–T20 | Jobs, Retention, Migration, kontrollierte Live-Abnahme und Rollback |

Abhängigkeiten: `T01 → T02 → T03/T04 → T05 → T06/T07 → T08 → T09/T10 → T11/T12 → T13/T14 → T15/T16`. T17 baut auf T03 und dem bestehenden Tenant-Service auf. T18 integriert T09–T12, T19 nutzt das fertige Schema, T20 benötigt alle Ergebnisse. Der explizite Cross-Task-Regressionstest T03→T05 wird in T05 geschrieben und ausgeführt. Tasks dürfen separat reviewed werden; zusammenhängende Schnittstellen dürfen nicht in mehreren inkompatiblen Varianten entstehen.

Grobe Arbeitsschätzung für eine mit dem Repo vertraute Person, **keine Zusage**: A 4–6, B 6–9, C 5–8, D 5–8, E 4–6 konzentrierte Entwicklungstage. Insgesamt 24–37 Tage inklusive Tests/Review; Providerzugang, DNS-Propagation und Freigaben zusätzlich. Bereits vorhandener Relaykit-Code reduziert Implementierungsarbeit, ersetzt jedoch keine Anpassung an die Webdock-Grenzen.

## 2. Codebasis und Wiederverwendung

Webdock-Basis: `73733b0` plus vorhandene lokale Änderungen. Relaykit-Referenz: `/home/newt/Projekte/Personal/turboSMTP/relaykit` bei `dfaa885`. Vor Ausführung erneut Status prüfen; dieser Plan darf fremde Änderungen weder revertieren noch beiläufig committen.

| Relaykit-Datei | Verwendung in Webdock | Erforderliche Anpassung |
| --- | --- | --- |
| `src/lib/emails.ts` und Tests | Validierung, normalisierte Mailfelder, Simulator | Eigene Grenzen/Branding, Snowflake-IDs, präzise Größenprüfung |
| `src/lib/sending.ts` | Versandablauf und Race-Testfälle | Verbindung pro Account, Recipient-Projektion, pg-Transaktionen, keine globale Mailerinstanz |
| `src/lib/api.ts` | Fehler-/Idempotenz-Verträge | Bounded Body, aktuelle Tenantprüfung, kanonischer Hash, dauerhaftes Batchmodell |
| `src/lib/delivery-events.ts` | Verlustfreie IDs und Payloadnormalisierung | Quelle+Connection, Inbox, vollständige nachgewiesene Providerstatus |
| `src/lib/webhook-{signing,transport}.ts` | Signatur-/SSRF-Verhalten mit Tests | Secrets verschlüsseln; Logredaktion; Deadline und Lease-Fencing |
| `src/lib/{webhooks,suppressions,logs}.ts` | Fachverhalten | Tenant-/Connection-Scope, Webdock-DB, Bcc-/Bodyminimierung |
| `src/lib/analytics.ts` | Datums-/Chartdarstellung | Lokale Events als einzige Quelle; explizite Empfängerdenominatoren |
| `src/lib/{openapi,webhook-example}.ts` | API-Dokumentation und Beispiele | Neue Basis-URL, `wdm_`, Limits und 202-Batch-Erweiterung |
| `src/app/(dashboard)/**` | Seitenumfang und Interaktionen | Webdock-Komponenten/i18n/SSO, keine zweite Shell |
| `src/lib/{auth,organizations}.ts`, `src/db/**` | Nur fachlicher Vergleich | Nicht kopieren: vorhandene Auth und pg verwenden |
| `src/lib/cloudflare.ts`, `src/app/api/cloudflare/**` | Keine Übernahme | Benutzer hat CF ausdrücklich ausgeschlossen |

Die lokale Relaykit-Referenz bleibt unverändert. Übernommene Implementierungsteile werden im neuen Modul mit Herkunft/Commit dokumentiert; Lizenz-/Eigentumsannahmen vor großflächiger Übernahme prüfen.

### Pfadkürzel für die folgenden Tasks

Diese Kürzel bezeichnen exakt folgende Verzeichnisse, keine zusätzliche Struktur:

- `CORE` = `apps/auth/src/lib/mail-product`
- `API` = `apps/auth/src/app/api/mail/v1`
- `UI` = `apps/admin/src/app/(console)/(portal)/tenants/[customerID]/mail`
- `COMP` = `apps/admin/src/components/mail`
- `TEST` = `apps/auth/tests`
- `CONTRACTS` = `packages/mail-contracts/src`

### Verantwortungsgrenzen

| Bereich | Dateien |
| --- | --- |
| Gemeinsame Verträge | `CONTRACTS/index.ts`, `validation.ts`, `limits.ts`, `events.ts` |
| Persistenz | `CORE/schema.ts`, `store.ts`, `apps/auth/scripts/migrate-mail-product.ts` |
| Rechte/Keys | `CORE/access.ts`, `api-keys.ts`, `apps/auth/src/lib/tenant-preview.ts` |
| Verbindungen | `CORE/connections.ts`, `provider-credentials.ts`; vorhandene `tenant-mail.ts`/`hosting/byok-mail.ts` |
| Domains | `CORE/domains.ts` als Verbindungseinstieg; vorhandene `mail-domains.ts`/`mail-tracking.ts`/`mail-dns.ts` |
| Ereignisse/Sperren | `CORE/events.ts`, `projections.ts`, `suppressions.ts` |
| Annahme/Dispatch | `CORE/idempotency.ts`, `quota.ts`, `rate-limit.ts`, `sending.ts`, `turbo-send.ts`, `content.ts` |
| Spätere Arbeit | `CORE/schedules.ts`, `batches.ts`, `callback-inbox.ts`, `reconcile.ts`, `jobs.ts` |
| Kunden-Webhooks | `CORE/webhooks.ts`, `webhook-signing.ts`, `webhook-transport.ts` |
| Lesen/Betrieb | `CORE/messages.ts`, `analytics.ts`, `request-logs.ts`, `retention.ts`, `health.ts` |
| API/Bridge | `CORE/http.ts`, `openapi.ts`, `studio.ts`; dünne Route Handler und bestehende Studio-Bridge |
| Studio | `COMP/**`, `UI/**`, `apps/admin/src/lib/mail-product.ts` |

Keine vollständige generische Repository-/Providerabstraktion entwickeln. `pg`-Queries und kleine fachliche Funktionen reichen. Provider-Sending hat genau einen turboSMTP-Adapter.

## 3. Gemeinsame Schnittstellen

Diese Typen/Bezeichnungen werden in T02 verbindlich angelegt. Interne Actors stammen ausschließlich aus T03 bzw. aus internen Jobclaims, niemals aus clientseitig gelieferten Rollen.

```ts
export type ID = string; // Snowflake; Provider-/Legacy-IDs separat als String
export type Permission = "sending_access" | "full_access";
export type MailCapability = "read" | "send" | "manage" | "operate";
export type MailActor = {
  customerID: ID;
  connectionID: ID;
  principal:
    | { kind: "session"; userID: ID; sessionID: ID }
    | { kind: "api_key"; keyID: ID }
    | { kind: "job"; jobID: ID };
};
export type EmailRequest = {
  from: string; to: string | string[]; subject: string;
  cc?: string | string[]; bcc?: string | string[];
  reply_to?: string | string[]; html?: string; text?: string;
  headers?: Record<string, string>;
  tags?: { name: string; value: string }[];
  attachments?: {
    content: string; filename?: string;
    content_type?: string; content_id?: string;
  }[];
  scheduled_at?: string;
};
export type ApiResult<T = unknown> = { status: number; body: T };
export type PageQuery = { limit?: number; after?: string };
export type SendResult = { id: ID };
export type MailErrorBody = {
  statusCode: number; name: string; message: string; request_id: string;
  email_id?: ID; failed_index?: number; failed_email_id?: ID | null;
};
export type BatchResult = {
  id: ID; status: "processing" | "succeeded" | "failed";
  data: SendResult[]; failed_index?: number; failed_email_id?: ID | null;
};
export type DispatchOutcome =
  | { kind: "accepted"; mid: string }
  | { kind: "rejected"; code: string }
  | { kind: "unknown"; code: string };
export type EventType =
  | "email.scheduled" | "email.canceled" | "email.sent"
  | "email.delivery_delayed" | "email.delivered" | "email.opened"
  | "email.clicked" | "email.bounced" | "email.complained"
  | "email.unsubscribed" | "email.failed" | "email.suppressed"
  | "domain.created" | "domain.updated" | "domain.deleted";
export type MailEventInput = {
  customerID: ID; connectionID: ID; resourceID: ID;
  type: EventType; recipient?: string; occurredAt: string;
  source: "application" | "provider" | "simulator" | "reconciliation";
  dedupeKey: string; details?: Record<string, unknown>;
};
```

Öffentliche Boundary validiert Strings/Objekte zur Laufzeit. Ein Type-Cast auf `MailActor` oder `EmailRequest` ersetzt weder Authentifizierung noch Validierung.

### Testkonvention

Jeder Task ergänzt aussagekräftige Tests, führt sie zuerst gegen den noch nicht implementierten Pfad aus, implementiert den Pfad und wiederholt nur betroffene Checks. Reine Darstellungsänderungen brauchen keine Tests, die nur den JSX-Aufbau nachzeichnen.

Die SQL-Tests verwenden in T01 den Helper `withMailFixture` aus `TEST/helpers/mail-product.ts`. Er erstellt getrennte Tenant-/User-/Connectiondaten in einer lokalen disponiblen Datenbank, echte Authsessions, kontrollierte Provider-Fakes und eine feste Uhr. Er darf keine Zugriffsprüfung durch überschreibbare `isAdmin`-Flags umgehen.

```ts
// Vertrag des T01-Testhelpers; konkrete Request-/Servicepfade werden integriert.
type Fixture = {
  db: import("pg").Pool;
  actorA: MailActor; actorB: MailActor;
  adminHeaders: Headers; memberHeaders: Headers; operatorHeaders: Headers;
  domainA: { id: ID; name: string };
  now: Date;
  setNow(instant: Date): void;
  provider: {
    calls: unknown[];
    reply: DispatchOutcome;
  };
  send(body: EmailRequest, idempotencyKey: string): Promise<ApiResult>;
  request(path: string, init?: RequestInit): Promise<Response>;
};
// Diese Hilfsfunktionen werden in T01/T02 implementiert und überall importiert.
declare function withMailFixture(run: (f: Fixture) => Promise<void>): Promise<void>;
declare function validEmail(overrides?: Partial<EmailRequest>): EmailRequest;
```

`f.send` ruft spätestens ab T08 die echte HTTP-/Service-Annahmekette auf; `f.request` ruft die echten Route Handler auf. Provider/DNS sind gestubbt, fachliche Services und PostgreSQL nicht. Vor fertiggestellter Route werden die dokumentierten Servicefunktionen direkt getestet; keine Mock-Resultate als Produktbeweis.

## T01: Voraussetzungen und isolierte Testbasis

**Files:** Create `TEST/helpers/mail-product.ts`, `TEST/mail-product-fixture.test.ts`, `TEST/fixtures/mail-provider-contracts.json`, `docs/verification/2026-10-12-mail-provider-contracts.md`. Modify `apps/auth/package.json`. Read `apps/auth/scripts/{provision.mjs,migrate.ts,migrate-tenants.ts,migrate-platform.ts,migrate-mail.ts,migrate-plans.ts}`.

**Interfaces:** Produces `withMailFixture`/`validEmail` gemäß Abschnitt 3; Provider-Contract-Fixtures für die nächsten Tasks. Keine produktiven Provider-Änderungen.

- [ ] Arbeitsbaum und anwendbare AGENTS.md prüfen; isolierten Umsetzungsbranch erstellen, dessen Ausgangspunkt die benötigten vorhandenen Änderungen enthält. Keine fremden Dateien in Taskcommits aufnehmen.
- [ ] Lokale Testdatenbank auf `127.0.0.1:5438/webdock_admin_test` bereitstellen bzw. vorhandene eindeutig disposable Instanz prüfen. Nie eine andere DB umbenennen/löschen, um einen Guard zu umgehen. Migrationsrollen, Snowflake-Funktion, Registry-/Auth-/Tenant-Schema in der bestehenden Reihenfolge initialisieren.
- [ ] Neuen Testsuite-Guard vor jedem Connect testen:

```ts
import assert from "node:assert/strict";
import test from "node:test";
test("mail tests refuse a remote or production database", () => {
  for (const url of [
    "postgresql://test:test@db.example.com/webdock_admin_test",
    "postgresql://test:test@127.0.0.1:5438/webdock",
  ]) assert.throws(() => assertMailTestDatabase(url));
});
```

- [ ] `assertMailTestDatabase(url: string): void` im Helper implementieren: Host localhost/127.0.0.1, DB exakt `webdock_admin_test`, `AUTH_TEST_MAIL=true`, keine Produktionsenv. Fixtures erzeugen eindeutige Kunden/Benutzer und räumen nur eigene Zeilen auf. Connection-/Messagefixtures kommen nach dem Schema in T02 hinzu; bis dahin werden keine noch nicht existierenden Produktdienste importiert. `setNow` steuert dieselbe injizierte Uhr für Idempotenz, Dispatch und Retention.
- [ ] `test:mail` als expliziten Runner für `tests/mail-product-*.test.ts` plus vorhandene Mail-/Turbo-Tests anlegen. Node-Test-Import benötigt `tsx`, Integration läuft mit `--test-concurrency=1`; die gezielten Race-Tests starten ihre konkurrierenden Operationen selbst.
- [ ] Die in Spec Abschnitt 12 genannten Providerfragen mit vorhandenen sanitized Belegen oder einem dafür freigegebenen Testkonto beantworten. Mindestens Consumer-Auth, Accountidentität, Callback-Ebene, Referenz-Echo, MID-Format, Statusaliases und Zähleinheit festhalten. Nicht durch diesen Plan autorisierter echter Versand ist kein Testsuite-Schritt.
- [ ] Run: `npm run test:mail -w @spitzli/auth`. Erwartung nach Integration der neuen Files: bestehende 21 reine Adaptertests und alle verfügbaren lokalen Integrationstests grün. Die bisher fehlende Datenbank als Umgebungsursache dokumentieren, nicht als Produktfehler verschleiern.
- [ ] Commit ausschließlich Fixture-/Testinfrastruktur mit `test(mail): establish isolated provider and tenant fixtures`.

**Abnahme:** Automatisierte Tests können ohne produktive Credentials laufen. Ungelöste Providerfragen sind konkrete Releaseblocker in der Nachweismatrix, keine erfundenen Antworten.

## T02: Verträge, SQL-Schema und additive Migration

**Files:** Create `packages/mail-contracts/package.json`, `CONTRACTS/{index,validation,limits,events}.ts`, `CORE/{schema,store}.ts`, `apps/auth/scripts/migrate-mail-product.ts`, `TEST/mail-product-schema.test.ts`. Modify `apps/auth/package.json`, `apps/admin/package.json`, Lockfile und `apps/auth/scripts/provision.mjs` nur für erforderliche Grants.

**Interfaces:** Produces Typen aus Abschnitt 3; `mailProductSchemaSQL: string`; `withMailTransaction<T>(run: (db: PoolClient) => Promise<T>): Promise<T>`. Bestehende `database`-Poolinstanz weiterverwenden.

- [ ] Tabellen aus Spec Abschnitt 6 anlegen, mit nachweisbarer Connection-/Tenant-FK-Konsistenz. Domainproof bleibt in `mail_sender_domain`. PK/FK-Felder verwenden den bestehenden Text-/varchar-Typ konsistent.
- [ ] Test für zweimalige Migration und Cross-Tenant-FK-Verstoß schreiben:

```ts
test("message cannot reference another tenant's connection", async () => {
  await withMailFixture(async f => {
    await f.db.query(mailProductSchemaSQL);
    await f.db.query(mailProductSchemaSQL);
    await assert.rejects(f.db.query(
      "INSERT INTO webdock_auth.mail_message(customer_id,connection_id,domain_id,from_address,subject,dispatch_status,scheduled_at,provider_reference) VALUES($1,$2,$3,$4,'x','scheduled',now()+interval '1 hour',$5)",
      [f.actorA.customerID, f.actorB.connectionID, f.domainA.id,
       "mail@" + f.domainA.name, randomUUID()],
    ), { code: "23503" });
  });
});
```

- [ ] Die im Test genannten Message-Spalten als Pflichtfelder der jeweiligen Dispatchebene definieren; `id,created_at,revision` erhalten Defaults. Der gezielte falsche Connection-FK muss die Fehlerursache sein. SQL-Kern:

```sql
-- In mail_connection:
UNIQUE (customer_id, id)
-- In mail_message:
FOREIGN KEY (customer_id, connection_id)
  REFERENCES webdock_auth.mail_connection(customer_id, id)
-- Provider-MIDs sind verbindungsgebunden:
CREATE UNIQUE INDEX mail_message_provider_mid_unique
ON webdock_auth.mail_message(connection_id, provider_mid)
WHERE provider_mid IS NOT NULL;
```

- [ ] Schema durch versionierte Migration mit Checksumme und explizitem Zielreport anwenden; keine Migration beim Appstart/build. Additive Backfills zunächst ohne Provideraufruf; Rollback löscht keine Mailtabellen.
- [ ] Vertragsvalidierung inklusive IDs, Eventnamen, Limits und Fehler-DTOs schreiben. `api_keys.domain_id` darf bei Löschung nicht durch `SET NULL` entschränkt werden; Domain-Tombstones verwenden.
- [ ] Indizes für Listen, Fälligkeit, Lease und Unique-Event/Delivery-Dedupe mit `EXPLAIN` an repräsentativen Fixtures prüfen. Runtime-Datenbankrolle darf neue DML ausführen, kein DDL.
- [ ] Run: `node --env-file=.env.test.local --import tsx --test tests/mail-product-schema.test.ts` in `apps/auth`; danach `npm run check -w @spitzli/auth` und `npm run check -w @spitzli/admin`.
- [ ] Commit: `feat(mail): add tenant-scoped mail contracts and schema`.

**Abnahme:** Wiederholbare additive Migration; keine Datenverschiebung oder neue externe Ressource.

## T03: Aktuelle Berechtigungen und eigene Webdock-Mail-Keys

**Files:** Create `CORE/{access,api-keys}.ts`, `TEST/mail-product-access.test.ts`, `TEST/mail-product-api-keys.test.ts`. Modify `apps/auth/src/lib/tenant-preview.ts` nur für explizite Ablehnung neuer Operationen, `apps/admin/src/lib/studio-contracts.ts` für DTO-Exports.

**Interfaces:** `authorizeMailSession(headers,customerID,connectionID,capability): Promise<MailActor>`; `authorizeMailKey(request,permission): Promise<MailActor>`; `assertMailAccess(actor,capability): Promise<void>`; `createMailKey(actor,{name,permission,domainID?}): Promise<{id,key,prefix}>`; `revokeMailKey(actor,id): Promise<void>`.

- [ ] Zweiten Tenant, Member/Admin/Operator, archivierten Kunden und aktive/abgelaufene Kundenansicht testen. Sending-Key darf nur senden; Full-Key darf keine Provider-Credentials, Kontingente oder Teamrollen ändern.
- [ ] In T03 Keywiderruf und direkt gespeicherte Domainrestriktion testen. Den folgenden zusätzlich notwendigen Regressionsfall verbindlich in T05 ausführen, sobald `assertVerifiedSender` existiert:

```ts
test("removing a restricted domain never widens a key", async () => {
  await withMailFixture(async f => {
    const key = await createMailKey(f.actorA, {
      name: "orders", permission: "sending_access", domainID: f.domainA.id,
    });
    await f.db.query(
      "UPDATE webdock_auth.mail_sender_domain SET deleted_at=now() WHERE id=$1",
      [f.domainA.id],
    );
    const request = new Request("https://auth.webdock.dev/api/mail/v1/emails", {
      headers: { Authorization: "Bearer " + key.key },
    });
    const actor = await authorizeMailKey(request, "sending_access");
    await assert.rejects(assertVerifiedSender(actor, "another.example"));
  });
});
```

- [ ] Keys mit `randomBytes(32)` erzeugen, `wdm_` voranstellen, Hash/Präfix persistieren; jede Antwort außer Create enthält nur Metadaten. Inputdaten niemals als Key-ID/Customer-Authority verwenden.
- [ ] Aktuelle Membership, User-Sperre/Verifizierung/Sicherheitssetup, Tenantstatus, Feature-Gates und Connectionstatus serverseitig prüfen. API-Keys an Tenant/Connection, nicht an Lebenszeit des Erstellers binden. `last_used_at` höchstens einmal pro Minute aktualisieren, damit jeder GET keine Schreiblast erzeugt.
- [ ] Revocation sofort wirksam; Rotation = neuen Key ausgeben, App umstellen, alten widerrufen. Bei API-Key-Erstellung über Full-Key Verbindung/Autorität nicht erweitern.
- [ ] Run: neue Access-/Keytests plus bestehende `tests/tenant-preview.test.ts` und `tests/studio-api.test.ts`.
- [ ] Commit: `feat(mail): authorize tenant mail API keys and live access`.

**Abnahme:** Fremde IDs ergeben 404; Secrets erscheinen weder in Logs noch DTOs; Preview kann keinen versteckten Schreibweg nutzen.

## T04: Plattform-/BYOK-Verbindungen und interne Sendesecrets

**Files:** Create `CORE/{connections,provider-credentials}.ts`, `TEST/mail-product-connections.test.ts`. Modify `apps/auth/src/lib/{tenant-mail,mail-keys,turbosmtp}.ts` und `apps/auth/src/lib/hosting/byok-mail.ts`.

**Interfaces:** `getMailConnections(actor): Promise<ConnectionView[]>`; `resolveSendingCredential(actor): Promise<{consumerKey,consumerSecret,region}>` nur serverintern; `ensureManagedSendingCredential(actor): Promise<void>`; `pauseMailConnection(actor): Promise<void>`. `ConnectionView` enthält IDs/Kind/Region/Status/Revision/Key-Suffix, keine Geheimnisse.

- [ ] Plattformunterkonto und BYOK parallel seedieren. Fake-Provider prüft exact account ID und markiert Consumer-Paare. Vorher getroffene tenantweite Auswahl darf weder Key noch Schedule umhängen.
- [ ] Test für zwei Schlüsselpaare desselben Accounts:

```ts
test("a provider account cannot be assigned to two customers", async () => {
  await withMailFixture(async f => {
    await f.db.query(
      "UPDATE webdock_auth.mail_connection SET provider_account_id='provider-42' WHERE id=$1",
      [f.actorA.connectionID],
    );
    await assert.rejects(f.db.query(
      "UPDATE webdock_auth.mail_connection SET provider_account_id='provider-42' WHERE id=$1",
      [f.actorB.connectionID],
    ), { code: "23505" });
  });
});
```

- [ ] Kontrollierten Key-Lifecycle implementieren: lokale Operation reservieren → genaues Childkonto autorisieren → `SEND_API`-Consumer-Paar erzeugen → purposegebunden verschlüsseln → Readiness setzen. Timeout/DB-Write-Fehler führt zu `needs_review`; markiertes Provider-Key-Label für Abgleich, kein automatischer Create-Retry.
- [ ] Bestehendes `encryptMailSecret/decryptMailSecret` für kleine Provider-/Webhook-Secrets verwenden; Purpose `mail-send:<customerID>:<connectionID>:<version>`. Rotation mit alter Version bis zur erfolgreichen neuen Speicherung, anschließend kontrollierter Widerruf.
- [ ] Interne Providerkeys aus der normalen Kundenliste ausschließen und deren Widerruf durch `manageTenantMailKeys` unterbinden; Plattformoperator kann den ausdrücklich als intern gekennzeichneten Lifecycle bedienen. BYOK-Paar vor Nutzung auf tatsächliche Accountidentität/Sendeberechtigung prüfen.
- [ ] Disconnect/Pause beendet neue Dispatches, erhält Verbindung/History und erlaubt Callback-Zuordnung. Bereits laufende Requests als solche kenntlich machen. Gleicher Account mit rotierten Credentials behält ID, anderer Account bekommt neue ID.
- [ ] Run: `mail-product-connections.test.ts` plus `mail-keys.test.ts`/`tenant-mail.test.ts` und die vorhandenen BYOK-Mail-Fälle aus `hosting.test.ts`.
- [ ] Commit: `feat(mail): bind durable sending credentials to provider accounts`.

**Abnahme:** Ein Kundenrequest verwendet ausschließlich das passende Accountpaar; bestehende direkte SMTP-Keys funktionieren weiter.

## T05: Gemeinsame Domain-/Trackingdienste mit manuellem DNS

**Files:** Create `CORE/domains.ts`, `TEST/mail-product-domains.test.ts`. Modify `apps/auth/src/lib/{mail-domains,mail-tracking,mail-dns,tenant-mail}.ts` und `hosting/byok-mail.ts`.

**Interfaces:** `assertVerifiedSender(actor,domain): Promise<{domainID: ID}>`; `claimMailDomain(actor,name): Promise<{id: ID}>`; `verifyMailDomain(actor,id): Promise<DomainView>`; `removeMailDomain(actor,id): Promise<void>`. `DomainView` enthält Connection/Ownership-/Providerflags/DNS-Records und Änderungshinweise.

- [ ] Vorhandene Sender-/Trackingtests zunächst unverändert grün halten; Plattform-/BYOK-Mapping auf `mail_connection_domain` ergänzen. Alte UI-Mutationen und neue API dürfen nicht zwei unabhängige Providerreservierungen führen.
- [ ] Sender-Autorisierung als Schnittmenge prüfen:

```ts
test("provider verification without local ownership cannot send", async () => {
  await withMailFixture(async f => {
    await f.db.query(
      "UPDATE webdock_auth.mail_sender_domain SET status='pending' WHERE id=$1",
      [f.domainA.id],
    );
    await assert.rejects(assertVerifiedSender(f.actorA, f.domainA.name));
    assert.equal(f.provider.calls.length, 0);
  });
});
```

- [ ] Ownership-TXT und Provider-SPF/DKIM/DMARC getrennt zeigen. Frische Providerbeobachtung höchstens fünf Minuten wiederverwenden; vor Handoff veraltete Daten über den verifizierten Account aktualisieren. Provider-Ausfall erlaubt keinen Versand mit unbekannter Verifizierung.
- [ ] Domainlöschung ist zunächst lokales Deaktivieren/Tombstone; API löscht kein DNS und nicht automatisch Providerregistrierungen, die Direkt-SMTP nutzen könnte. Explizites „Providerregistrierung entfernen“ bleibt separater bestätigter Verwaltungsweg mit Tracking-/Schedule-Hinweis.
- [ ] Einschränkungen existierender Keys bleiben auf tombstoned Domain-ID stehen. Vorhandene Unique-Indizes und alle Leser auf `deleted_at IS NULL` für aktive Claims umstellen; neue Registrierung erhält frische ID/Proof, keine Wiederbelebung alter eingeschränkter Keys. Schedules mit gelöschter Domain schlagen beim Dispatch nachvollziehbar fehl. Den in T03 beschriebenen Domainlöschungs-Regressionsfall hier implementieren und ausführen.
- [ ] BYOK-Domains vor Gatewayversand durch denselben lokalen TXT-Proof führen. Vorhandene Providerregistrierung allein reicht nicht. Custom Tracking verwendet denselben Connection-Scope und vorhandene sichere Reservierungs-/Reconciliationlogik.
- [ ] Keine Cloudflare-OAuth-Routen, DNS-API-Schreibclients oder neue CF-Secrets erstellen. Vorhandene öffentliche DNS-Leseprüfung ist erlaubt; Copy-and-check funktioniert mit jedem DNS-Hoster.
- [ ] Run: `mail-product-domains.test.ts`, `mail-domains.test.ts`, `mail-tracking.test.ts`, `turbo-domains.test.ts`, `turbo-tracking.test.ts`.
- [ ] Commit: `feat(mail): unify connection-scoped domains and manual DNS onboarding`.

**Abnahme:** Zwei Connections desselben Tenants sind eindeutig; andere Tenants können weder Sender- noch Trackingregistrierungen übernehmen.

## T06: Kanonische Events, Empfängerzustände und Sperrlisten

**Files:** Create `CORE/{events,projections,suppressions}.ts`, `TEST/mail-product-events.test.ts`, `TEST/mail-product-suppressions.test.ts`. Modify `CONTRACTS/events.ts`.

**Interfaces:** `recordMailEvent(event: MailEventInput): Promise<{inserted: boolean}>`; `getSuppressedRecipients(customerID,emails: string[]): Promise<Set<string>>`; `setMailSuppression(actor,{email,reason}): Promise<void>`; `removeMailSuppression(actor,email): Promise<void>`; `projectRecipient(previous,event): RecipientState` als pure Funktion.

- [ ] Testzustände für `sent → delivered → delayed`, `clicked` vor `delivered`, mehrere Empfänger, Bcc und doppelte Events anlegen. `RecipientState` in Contracts mit `delivery,firstDeliveredAt,firstOpenedAt,lastOpenedAt,firstClickedAt,lastClickedAt,complainedAt,unsubscribedAt` definieren; Zeitpunkte nullable.
- [ ] Reine Projektion testen:

```ts
test("late acceptance does not regress a delivered recipient", () => {
  const delivered = projectRecipient(emptyRecipientState(), {
    type: "email.delivered", occurredAt: "2026-10-12T12:00:00Z",
  });
  const late = projectRecipient(delivered, {
    type: "email.sent", occurredAt: "2026-10-12T11:59:59Z",
  });
  assert.equal(late.delivery, "delivered");
  assert.equal(late.firstDeliveredAt, "2026-10-12T12:00:00Z");
});
```

- [ ] `emptyRecipientState(): RecipientState` bereitstellen. `projectRecipient` nimmt `Pick<MailEventInput,"type"|"occurredAt">` an. Delivery und Engagement nicht in einen einzelnen Rang pressen; serverseitigen Empfangszeitpunkt getrennt speichern.
- [ ] Eventinsert mit eindeutiger Dedupe-ID, Empfängerprojektion, terminaler Sperre und Outbox-Inserts in **einer** DB-Transaktion ausführen. Bei Duplicate muss auch die Outbox unverändert bleiben:

```sql
INSERT INTO webdock_auth.mail_event
  (customer_id,connection_id,resource_id,event_type,dedupe_key,occurred_at)
VALUES ($1,$2,$3,$4,$5,$6)
ON CONFLICT (dedupe_key) DO NOTHING
RETURNING id;
-- Nur bei RETURNING-Zeile Projektion/Suppression/Outbox fortschreiben.
```

- [ ] `email.bounced`/`complained`/`unsubscribed` sperren normale Empfänger tenantweit. `delivery_delayed` und Simulatorereignisse nicht. Gründe priorisieren: Beschwerde/Abmeldung darf durch späteren manuellen/Bounce-Eintrag nicht verdeckt werden.
- [ ] Fremde Resource-/Recipient-ID, event time jenseits einer begrenzten plausiblen Zukunft und unbekannter Eventtyp werden abgelehnt/quarantänisiert. Eventdetails nur aus typisierter Allowlist, niemals Provider-Payload ungefiltert.
- [ ] Parallel denselben Event zweimal schreiben: ein Event, eine Sperre und je Endpoint genau eine Outbox-Zeile. Provider-Suppression-Readback bleibt Connection-gebunden; lokales DELETE ruft kein Provider-DELETE auf.
- [ ] Run: `node --env-file=.env.test.local --import tsx --test tests/mail-product-events.test.ts tests/mail-product-suppressions.test.ts`.
- [ ] Commit: `feat(mail): persist recipient events and tenant suppressions atomically`.

**Abnahme:** Die Eventprojektion ist die gemeinsame Grundlage für Versanddetails, Analytics und Webhooks; keine verlorenen Outbox-Einträge bei Crash.

## T07: Idempotenz, Budget, Rate-Limits und Request-Logs

**Files:** Create `CORE/{idempotency,quota,rate-limit,request-logs,http}.ts`, `TEST/mail-product-idempotency.test.ts`, `mail-product-quota.test.ts`, `mail-product-logs.test.ts`. Modify `apps/auth/src/lib/plans.ts` nur zum Export des bestehenden autorisierten/effective-Allowance-Lesepfads für Systemarbeit.

**Interfaces:** `runIdempotent(actor,{key,method,path,body},run): Promise<ApiResult>`; `reserveMailQuota(actor,messageID,recipientCount,at): Promise<void>`; `settleMailQuota(messageID,outcome): Promise<void>` mit Outcome `accepted/rejected/unknown`; `consumeMailRate(actor,{requests,recipients,keys}): Promise<void>`; `redactMailLog(value): unknown`; `readBoundedJson(request,maxBytes): Promise<unknown>`.

- [ ] Concurrent-Request-Test ohne Provider schreiben:

```ts
test("same idempotency key runs the operation once", async () => {
  await withMailFixture(async f => {
    let calls = 0;
    const input = { key: "order-42", method: "POST", path: "/emails", body: { x: 1 } };
    const run = async () => { calls++; return { status: 200, body: { id: "42" } }; };
    const results = await Promise.all([
      runIdempotent(f.actorA, input, run),
      runIdempotent(f.actorA, input, run),
    ]);
    assert.equal(calls, 1);
    assert.ok(results.every(r => r.status === 200 || r.status === 409));
    assert.deepEqual(await runIdempotent(f.actorA, input, run), {
      status: 200, body: { id: "42" },
    });
  });
});
```

- [ ] Methode+normierter Pfad+sortierte Objektkeys hashen; Arrays nicht sortieren. Idempotenz-Key 1–256 druckbare Zeichen. Gleicher Key mit anderem Body gibt 409. Gespeicherte Erfolgs-/Fehlerantwort wird exakt wiedergegeben. Auth/Revocation vor Replay prüfen, zeitabhängige Businessvalidierung danach.
- [ ] Reservationsinsert/Message-Zuordnung atomar machen; endgültige Antwort als separaten Commit speichern. Scheitert dieser nach Handoff, bleibt der Vorgang blockiert. Abgelaufene **abgeschlossene** Reservation per Bedingung atomar ersetzen; offene Unknowns nicht löschen.
- [ ] Budget im Transaktionslock pro Tenant+UTC-Monat reservieren, Tenant/Planrevision prüfen. `0`, `null`, kein Plan, Monatswechsel, zwei parallele Reservierungen für den letzten freien Empfänger und unbekannte Handoffs testen. Settlement idempotent pro Message, auch bei Callback vor Sendantwort.
- [ ] Alle Gateway-Verbindungen summieren sich im lokalen Tenantbudget; Providerbudget separat. Simulatordaten belasten es nicht. Ratenzähler benutzen DB-Zeit und atomare UPSERTs; 429 mit `Retry-After`, keine unbegrenzte In-Memory-Map.
- [ ] Stream einmal bounded einlesen; falscher Content-Type 415, zu groß 413, ungültiges JSON 422. Das Logging verwendet die bereits validierte Kopie und maximal 32 KiB redigierte Metadaten.
- [ ] Secrets rekursiv redigieren; `html/text` durch Größen/Hashes ersetzen; Bcc nicht in Logs. Teststrings für ConsumerSecret, AccessToken, verschachtelte Signatursecrets, große Attachment-Base64, URL-Query-Credentials und Providerfehlermeldungen dürfen in gespeicherten Logs nicht auftauchen.
- [ ] Logdatensatz mit Request-ID bei authentifizierter Annahme anlegen; Ergebnis/Dauer best effort aktualisieren. `unfinished` nach Crash ist erlaubt und sichtbar; Logfehler darf keine Wiederholung eines angenommenen Sends auslösen. Sicherheits-Audit für Key/Connection-Änderungen bleibt gesondert.
- [ ] Run: neue Idempotenz-/Quota-/Logtests und `tests/plans.test.ts`.
- [ ] Commit: `feat(mail): add atomic send budgets idempotency and redacted logs`.

**Abnahme:** Wiederholung, Parallelität und Logging können keinen zusätzlichen Provider-Send verursachen.

## T08: Einzelversand, Anhänge und Simulator

**Files:** Create `CORE/{sending,turbo-send,content}.ts`, `TEST/mail-product-send.test.ts`, `mail-product-content.test.ts`, `mail-product-simulator.test.ts`. Modify `CONTRACTS/validation.ts`.

**Interfaces:** `submitMail(actor,body: EmailRequest): Promise<ApiResult<SendResult | MailErrorBody>>`; `dispatchMail(messageID,deadline): Promise<DispatchOutcome>`; `sendTurboMail(credential,body,reference,signal): Promise<DispatchOutcome>`; `eraseAttachmentContent(messageID): Promise<void>`.

- [ ] Relaykit-Validator-Testfälle übernehmen, dann zusätzliche Größen-/Header-/Dedupe-Grenzen der Spec testen. `path`-/URL-Anhänge ausdrücklich zurückweisen. To/Cc/Bcc nicht mit unsicherem Komma-Split eines Anzeigenamens parsen.
- [ ] Verlust der Annahmeantwort testen:

```ts
test("unknown handoff is never sent again by a client retry", async () => {
  await withMailFixture(async f => {
    f.provider.reply = { kind: "unknown", code: "timeout" };
    const first = await f.send(validEmail(), "invoice-42");
    const second = await f.send(validEmail(), "invoice-42");
    assert.equal(first.status, 502);
    assert.equal((first.body as MailErrorBody).name, "send_outcome_unknown");
    assert.ok((first.body as MailErrorBody).email_id);
    assert.deepEqual(second, first);
    assert.equal(f.provider.calls.length, 1);
  });
});
```

- [ ] `f.send` jetzt an die reale Annahmekette anbinden. Reihenfolge aus Spec Abschnitt 7 umsetzen: validieren/Domain prüfen → Message persistieren → Sperren/Quota → Dispatchmarker committen → Provider → Settlement/Event. DB-Verbindung vor jedem Provider-HTTP-Aufruf freigeben.
- [ ] Nur feste EU-/Global-Origins des Accountbindings; Consumer-Key/Secret-Header, kein Master-Fallback. Referenz als kryptografisch zufälliger Wert speichern und mit `reference_id` schicken. Request-/Responsebody und Timeout begrenzen; Provider-MID verlustfrei parsen.
- [ ] Fehlerklassen konkret: dokumentierte definitive 400/401/403/422-Ablehnung führt zu `failed`; Timeout/5xx/unklare oder abgebrochene Antwort zu `outcome_unknown`. Rate-Limit-/sonstige Antworten nur als sicher nicht angenommen einstufen, wenn T01 dies belegt; sonst konservativ unbekannt.
- [ ] Attachment-Payload getrennt encrypten. Der vorhandene Secret-Decryptor hat eine 32-KiB-Grenze und darf dafür **nicht** unverändert genutzt oder global entgrenzt werden. `content.ts` verwendet AES-256-GCM, einen aus dem vorhandenen stabilen Secret abgeleiteten zweckgetrennten Key, Kunde+Message+Attachmentindex als AAD sowie eine eigene feste 4-MB-Ciphertextgrenze.
- [ ] Erfolgreiche Annahme entfernt Attachment-Base64, behält Filename/Type/CID/Größe. Inhalt nie in Logs/Webhooks/Listen. Fehler beim Bereinigen erzeugt nachholbare Arbeit, niemals erneuten Versand.
- [ ] Abschlussupdate an Versuchstoken und unbestätigten Dispatchstatus binden: Ein bereits zustellbestätigter Callback hat Vorrang vor einem verspäteten Timeout der ursprünglichen Sendanforderung. Quoten-Settlement und Bereinigung sind idempotent aus beiden Pfaden ausführbar.
- [ ] Simulator erzeugt lokale akzeptierte/delivered/bounced/complained Events im gleichen Pfad. Testfälle: drei Outcomes, keine Providercalls, keine echten Suppressions, kein Quotenverbrauch, gemischte echte/Testadressen 422.
- [ ] Tests für Providerannahme mit anschließendem DB-Fehler, nur unterdrückte To-Empfänger, teilweise unterdrückte Cc/Bcc, falsche Domain, verlorene Berechtigung unmittelbar vor Handoff ergänzen.
- [ ] Run: `node --env-file=.env.test.local --import tsx --test tests/mail-product-send.test.ts tests/mail-product-content.test.ts tests/mail-product-simulator.test.ts`.
- [ ] Commit: `feat(mail): implement durable transactional sending and simulator`.

**Abnahme:** Ein API-Aufruf liefert eine nachverfolgbare eigene Message-ID; „accepted/sent“ ist ausdrücklich nicht „delivered“.

## T09: Geplante Nachrichten, Verschieben und Abbruch

**Files:** Create `CORE/schedules.ts`, `TEST/mail-product-schedules.test.ts`. Modify `CORE/sending.ts` und `CONTRACTS/validation.ts`.

**Interfaces:** `dispatchDueMail({limit,deadline}): Promise<{processed:number}>`; `rescheduleMail(actor,id,scheduledAt,revision): Promise<void>`; `cancelScheduledMail(actor,id,revision): Promise<void>`.

- [ ] ISO-Zeit mit explizitem Offset, Kalenderfehler, DST, Vergangenheit und mehr als 30 Tage prüfen. Zeit intern in UTC speichern; Anzeige nutzt vorhandene i18n-Funktionen.
- [ ] Konkurrenz zwischen Abbruch und Dispatch testen; genau ein Pfad gewinnt:

```sql
UPDATE webdock_auth.mail_message
SET dispatch_status='canceled', revision=revision+1
WHERE id=$1 AND customer_id=$2 AND connection_id=$3
  AND dispatch_status='scheduled' AND revision=$4
RETURNING id;
```

- [ ] Due-Claim mit `FOR UPDATE SKIP LOCKED` in kurzer Transaktion umsetzen, dann Dispatchstatus/Versuchstoken committen. Bei abgearbeiteter Revision 409 statt Erfolg vortäuschen.
- [ ] Vor Versand erneut Tenantstatus, Connection, Domain, Budget und Feature-Gates prüfen. Widerruf des ursprünglichen Keys storniert einen bereits angenommenen Tenant-Schedule nicht; Tenant-/Domain-/Connectionentzug dagegen verhindert Dispatch.
- [ ] Feste Connection-ID bleibt trotz UI-Defaultwechsel erhalten. Pausierte Connections halten fällige Schedules mit `blocked_reason` zurück, zählen diese sichtbar und senden nach explizitem Fortsetzen; endgültig archivierte Tenants/deaktivierte Domains erzeugen terminalen Fehler. Budgetmangel beim Dispatch ist gemäß Spec terminal.
- [ ] Crash vor Providerstart und nach Providerstart getrennt testen. Nach dauerhaftem Dispatchmarker ohne nachgewiesenes Ergebnis kein automatischer Neuversand. Operatorreconciliation darf einen rein lokalen, belegbar noch nicht gestarteten Versuch gezielt freigeben; kein Timeout allein ist dieser Beleg.
- [ ] Idempotenz-Replay eines gestern angenommenen Schedules darf heute nicht wegen `scheduled_at` in der Vergangenheit scheitern. Request-ID bleibt gleich, keine zweite Message.
- [ ] Run: `tests/mail-product-schedules.test.ts` inklusive paralleler Worker/Cancel/Reschedule-Fälle.
- [ ] Commit: `feat(mail): schedule reschedule and cancel durable messages safely`.

**Abnahme:** Kein Doppelversand bei zwei Tickinstanzen; jeder abgelehnte/gesperrte Schedule ist in Studio erklärbar.

## T10: Durable Batches und begrenzte Inline-Ausführung

**Files:** Create `CORE/batches.ts`, `TEST/mail-product-batches.test.ts`. Modify `CORE/{sending,idempotency}.ts`.

**Interfaces:** `submitMailBatch(actor,bodies: EmailRequest[]): Promise<ApiResult<BatchResult | MailErrorBody>>`; `advanceMailBatch(batchID,deadline): Promise<void>`; `getMailBatch(actor,id): Promise<BatchResult>`.

- [ ] Alle 1–100 Einträge und Domains vor dem ersten Send prüfen. Batchanhänge/Schedules 422; ungültiger Eintrag 50 führt zu null Providercalls. Atomare Batchanlage mit Reihenfolge und erforderlichem Budgetcheck, keine versteckten Teilannahmen bei Eingabefehlern.
- [ ] Teilfehler pinnen:

```ts
test("a failed second batch item preserves the first and stops the third", async () => {
  await withMailFixture(async f => {
    // Fake wechselt beim zweiten Send von accepted auf unknown.
    setProviderSequence(f, [
      { kind: "accepted", mid: "9007199254740993123" },
      { kind: "unknown", code: "timeout" },
    ]);
    const result = await submitMailBatch(f.actorA, [
      validEmail({ subject: "one" }), validEmail({ subject: "two" }),
      validEmail({ subject: "three" }),
    ]);
    assert.equal(result.status, 502);
    assert.equal((result.body as BatchResult).data.length, 1);
    assert.equal((result.body as BatchResult).failed_index, 1);
    assert.equal(f.provider.calls.length, 2);
  });
});
```

- [ ] `setProviderSequence(f,outcomes: DispatchOutcome[]): void` als kleine Fake-Erweiterung im T01-Helper definieren. Jede Message und ihre Batchposition vor Handoff speichern; Message-/OutcomeIDs müssen auch bei Fehlern erhalten bleiben.
- [ ] Höchstens 20 Sekunden Inlinearbeit, keinen neuen Providerrequest beginnen, wenn dessen 15-Sekunden-Budget nicht mehr hineinpasst. Rest bleibt durable und wird vom Dispatchjob verarbeitet. 202 liefert Batch-ID und bekannten Ergebnispräfix; `GET /batches/{id}` liefert Fortschritt.
- [ ] Batch kann pro Verbindung sequenziell laufen; Worker mit Lease/Token schützt dieselbe Position. Nie einen unbekannten Index überspringen und später fortsetzen. Folgeelemente nach terminalem/unknown Fehler werden `not_sent`.
- [ ] HTTP-Response für den Idempotenz-Key festhalten, auch 202. Polling mit Full-Key/Studio erklären; ein Sending-Key bekommt durch Batchstatus keine neue allgemeine Leseberechtigung.
- [ ] Tests für 100 langsame Sends, Workercrash, erneuten Aufruf mit gleichem Key, parallel gleiche Batchoperation, Pause nach Item 1 sowie fremde Batch-ID.
- [ ] Run: `tests/mail-product-batches.test.ts` plus Idempotenz-/Sendtests.
- [ ] Commit: `feat(mail): add durable bounded batch dispatch and progress`.

**Abnahme:** 100 Nachrichten können nicht durch Functiontimeout ungeplant doppelt verarbeitet werden; Teilresultate bleiben abrufbar.

## T11: Provider-Callbacks, Inbox und sichere Reconciliation

**Files:** Create `CORE/{callback-inbox,reconcile}.ts`, `apps/auth/src/app/api/mail/provider-events/[sourceID]/route.ts`, `TEST/mail-product-callbacks.test.ts`, `mail-product-reconcile.test.ts`. Modify `CORE/events.ts` nur über dessen bestehende Projektion.

**Interfaces:** `acceptProviderEvents(sourceID,request): Promise<Response>`; `processCallbackInbox({limit,deadline}): Promise<{processed:number}>`; `reconcileMail(actor,id): Promise<void>`. Providerparser in `callback-inbox.ts`: `parseProviderEvents(raw: string): NormalizedProviderEvent[]`.

- [ ] Quelle vor Bodyverarbeitung über Basic-Secret prüfen; gespeicherter Secret-Hash, konstante Vergleichszeit, keine Secretwerte in URL-/Requestlogs. `sourceID` ist Routing, keine Autorisierung.
- [ ] Parser mit Flat/Array, beiden Timestamp-Schreibweisen, dokumentierten Statusaliases und großen IDs testen:

```ts
test("callback numeric MID keeps every digit", () => {
  const [event] = parseProviderEvents(
    '{"mid":9007199254740993123,"status":"DELIVERED","email":"a@example.com","timestamp":1791806400}',
  );
  assert.equal(event.mid, "9007199254740993123");
});
```

- [ ] `NormalizedProviderEvent` definieren: `mid:string,providerEventID?:string,reference?:string,recipient:string,type:EventType,occurredAt:string,details:Record<string,unknown>`. Node-24-JSON-Reviver-Source für MID und Event-ID verwenden; keine bereits gerundeten Numbers mit `String(number)` „retten“.
- [ ] Body/Arraylimit und Quellrate durchsetzen, gültige Events durable annehmen und schnell 200 antworten. DB-Annahmefehler 503; gültige Provider-Replays dürfen erneut ankommen. Batch mit einzelnen unbrauchbaren Items separat quarantänisieren und gültige Items verarbeiten; komplett unlesbares JSON 406.
- [ ] Referenz/MID **und** erlaubte Source-Connection **und** gespeicherte Empfängerliste prüfen. Callback vor Providerantwort darf fehlende MID bedingt ergänzen; kollidierende bekannte MID darf nicht überschrieben werden. Gespeicherter Snapshot der tatsächlich gesendeten Empfänger verhindert Events für lokal supprimierte Adressen.
- [ ] Parent-Callback ohne Account-/Referenzkennung nur bei genau einem passenden bereits gespeicherten MID-/Empfängerkandidaten unter den erlaubten Connections zuordnen; mehrere Treffer quarantänisieren. Bestätigender Callback darf Annahme/Quoten-Settlement/Bereinigung vor dem Sendrequest abschließen, spätere Timeoutupdates dürfen das nicht zurücksetzen.
- [ ] Quarantäne/Unknownzähler begrenzen; keine fremden Betreff-/Bodydaten in die Kundenhistorie importieren. `REPORT/SPAM`, `UNSUBSCRIBED` und `PROCESSED` anhand T01 normieren, Unknownstatus sichtbar messen.
- [ ] Reconciliation nur für bekannte Message-IDs des genauen Accounts. Bounded Provider-Analytics-Lookup, exakte MID/Referenz+Empfängerprüfung; gefundene Beobachtungen in `recordMailEvent` einspeisen. Kein separater Statuscache und kein Resend.
- [ ] Zwei Provideraccounts mit gleicher MID, Callback-Fälschung über fremde Referenz, doppelte Opens, out-of-order Delivery, Event vor Sendantwort und DB-Fehler zwischen Inbox/Projektion testen.
- [ ] Run: Callback-/Reconciletests sowie T06-Eventsuite.
- [ ] Commit: `feat(mail): ingest scoped provider callbacks and reconcile unknown sends`.

**Abnahme:** Verifizierter Callback löst unbekannte Handoffs ohne erneute Zustellung auf. Callbackkonfiguration beim Provider bleibt ein separat nachzuweisender Betriebsschritt.

## T12: Signierte Kunden-Webhooks mit Retry und Replay

**Files:** Create `CORE/{webhooks,webhook-signing,webhook-transport}.ts`, `TEST/mail-product-webhooks.test.ts`, `mail-product-webhook-transport.test.ts`. Modify `apps/auth/package.json` nur falls `ipaddr.js` als direkte Dependency übernommen wird.

**Interfaces:** `createMailWebhook(actor,{url,events}): Promise<{id,secret}>`; `updateMailWebhook(actor,id,{url?,events?,enabled?}): Promise<void>`; `rotateMailWebhook(actor,id): Promise<{secret}>`; `replayMailWebhook(actor,deliveryID): Promise<void>`; `deliverDueWebhooks({limit,deadline}): Promise<{processed:number}>`; `signWebhook(secret,id,timestamp,body): string`.

- [ ] Signaturtest mit festem Vektor aus Relaykit portieren; zusätzliche Secret-Redaktions-/Tenanttests. Bodybytes werden einmal in der Outbox gespeichert; Retry verändert nur Timestamp/Signatur, nicht Event-ID oder Payload.
- [ ] Rebinding-/IPv6-Testfälle, keine Redirects, Credentials-in-URL, .local/.internal, gemischte öffentliche/private DNS-Antworten, langsamer Body, TLS-Hostname und Max-Response-Länge pinnen:

```ts
test("webhook targets reject non-public destinations", () => {
  for (const target of [
    "http://example.com/hook", "https://127.0.0.1/hook",
    "https://[::1]/hook", "https://user:pass@example.com/hook",
    "https://service.internal/hook", "https://example.com:8443/hook",
  ]) assert.notEqual(webhookEndpointProblem(target), null);
});
```

- [ ] `webhookEndpointProblem(url:string): string|null` exportieren. Transport benutzt gepinnte geprüfte DNS-Adresse und normale TLS-Zertifikatsprüfung. Kein `fetch` mit ungeprüftem Re-Resolve/Redirect.
- [ ] Unveränderliche Outbox mit `UNIQUE(webhook_id,event_id)`; Claim für 120 Sekunden, zufälliger Token. Abschlussupdate muss ID+Lease-Token treffen; ein alter Worker kann neue Attempts nicht überschreiben.
- [ ] 2xx = Erfolg; andere Codes/Netzwerkfehler = dokumentierter Retryplan. Höchstens initial+6 Retries. `attempt_count` wird beim begonnenen Versuch erhöht; Crash nach Empfang kann eine Doppelzustellung verursachen und ist als at-least-once dokumentiert.
- [ ] Disable pausiert neue Auswahl, Delete endet alle noch nicht begonnenen Versuche, Enable setzt Pending fort. Rotation liefert Secret einmal und verschlüsselt es; alle neuen Versuche inklusive Retries signieren mit aktueller Version. Keine Klartextsecrets in Listen/Logs.
- [ ] Manueller Replay nur eigener Delivery ohne aktive Lease, setzt Versuchszyklus zurück und erhält Event-ID. Empfängersample prüft Rawbody, Timestamptoleranz von fünf Minuten und atomare ID-Deduplizierung.
- [ ] Bcc und Mailbody fehlen in Eventpayloads; URLs/Antwortausschnitte vor Speicherung redigieren. Tenantarchive stoppt Ausgang, ohne Provider-Inbox zu stoppen.
- [ ] Run: Webhook-/Transporttests, T06-Outbox-Atomizitätstests.
- [ ] Commit: `feat(mail): deliver signed customer webhooks with bounded retries`.

**Abnahme:** Unzuverlässige Empfänger blockieren keinen Versandjob und können keine internen Netzwerkziele erreichen.

## T13: Nachrichtenabfragen und konsistente Analytics

**Files:** Create `CORE/{messages,analytics}.ts`, `TEST/mail-product-messages.test.ts`, `mail-product-analytics.test.ts`.

**Interfaces:** `listMailMessages(actor,query): Promise<{data:MessageSummary[],nextCursor:string|null}>`; `getMailMessage(actor,id): Promise<MessageDetail>`; `getMailAnalytics(actor,{days,domainID?,includeSimulated?}): Promise<MailAnalytics>`. DTOs in `CONTRACTS/index.ts` definieren; Summary enthält keinen Body/Bcc, Detail enthält nur für berechtigte Leser Envelope/Content/Timeline.

- [ ] Einheitliche Queries gegen Message-/Recipient-Projektion schreiben. Keine UI-Abfrage direkt gegen globale Provideranalytics. Dashboardliste, Detail und Kennzahlen verwenden dieselben Filterdefinitionen.
- [ ] Fixture: zwei echte Nachrichten, drei akzeptierte Empfänger, davon zwei delivered, einer bounced; zwei zusätzliche unterdrückte Empfänger; eine Simulatormail. Erwartung:

```ts
assert.equal(stats.messagesAccepted, 2);
assert.equal(stats.recipientsAccepted, 3);
assert.equal(stats.delivered, 2);
assert.equal(stats.bounced, 1);
assert.equal(stats.suppressed, 2);
assert.equal(stats.deliveryRate, 2 / 3);
assert.equal(stats.simulatedIncluded, false);
```

- [ ] `MailAnalytics` mit diesen Feldern plus opened/clicked/complained/unsubscribed, Raten, `days: {date,accepted,delivered,bounced}[]` und `asOf` definieren. Raten sind 0–1 oder null; für Open-/Clickrate zählt nur Engagement innerhalb der bestätigten delivered-Empfängerkohorte. Roh-Engagement ohne Delivery bleibt separat sichtbar.
- [ ] UTC-Kohorte nach Annahmezeit, Datum `from <= accepted_at < until`, 7/30/90 Tage; heutiger Tag enthalten, Tage ohne Versand mit Nullwerten. Später eintreffende Events aktualisieren die Kohorte ihrer Nachricht. Keine Doublecounts durch Joins auf wiederholte Opens.
- [ ] Cursor ist opaque, kodiert created_at+ID+Filterhash. Fremder/falscher Cursor 422; exakt gleiche Zeiten und neue Inserts zwischen Seiten dürfen keine Duplikate erzeugen. Limits 20/100; SQL-Filter vor Pagination.
- [ ] Suchfelder explizit auf ID, Betreff, Sender, erlaubte Empfänger begrenzen. Tenant-/Connectionfilter in jeder SQL-Abfrage. Keine vollständige Nachrichtensammlung in den Browser/`packages/search` laden.
- [ ] Body-/Eventretention sichtbar: abgelaufener Inhalt ist kein APIfehler; `content_available:false` mit Frist. Technische MIDs nur in berechtigten Detailansichten, nicht im öffentlichen Seitenindex.
- [ ] Run: Messages-/Analytics-Tests mit 90-Tage-Grenzen, DST-Anzeigen, Cross-Tenant-ID, leeren Daten, identischen Zeitstempeln, Simulatorfilter und Multi-Recipient-Mischstatus.
- [ ] Commit: `feat(mail): expose local message history and consistent analytics`.

**Abnahme:** Die Relation zwischen akzeptiert, zugestellt und geöffnet ist nachvollziehbar; der PDF-Befund „Liste und Analytics widersprechen sich“ ist reproduzierbar verhindert.

## T14: Öffentliche API, Fehlerverträge und OpenAPI

**Files:** Create `CORE/openapi.ts`, `API/openapi.json/route.ts`, `apps/auth/src/app/mail/docs/route.ts` und die unten genannten Handler. Test `TEST/mail-product-api.test.ts`, `mail-product-openapi.test.ts`. Modify `CORE/http.ts`.

**Handler-Dateien relativ zu API:**

```text
emails/route.ts
emails/batch/route.ts
emails/[id]/route.ts
emails/[id]/cancel/route.ts
batches/[id]/route.ts
domains/route.ts
domains/[id]/route.ts
domains/[id]/verify/route.ts
api-keys/route.ts
api-keys/[id]/route.ts
webhooks/route.ts
webhooks/[id]/route.ts
webhooks/[id]/deliveries/route.ts
webhooks/[id]/rotate/route.ts
webhooks/[id]/deliveries/[deliveryID]/replay/route.ts
suppressions/route.ts
suppressions/[email]/route.ts
logs/route.ts
logs/[id]/route.ts
analytics/route.ts
```

**Interfaces:** `mailEndpoint(permission,handler)` für Auth, Limits, Request-ID, Fehlerabbildung und Logs. Konkrete Handler delegieren auf T03–T13, ohne zweite Businesslogik.

- [ ] Vertragsmatrix aus Spec Abschnitt 9 als Tests pflegen: Methoden, Permissions, Statuscodes, Query-/Bodyschema, sichere Fehler und falsche IDs. Jede veröffentlichte OpenAPI-Operation muss eine registrierte Implementierung haben.
- [ ] Handler bleiben dünn, z. B.:

```ts
export const POST = mailEndpoint("sending_access", async (request, actor) => {
  const body = await readBoundedJson(request, 4_000_000);
  return runIdempotent(actor, {
    key: request.headers.get("idempotency-key"),
    method: "POST", path: "/emails", body,
  }, async () => submitMail(actor, parseEmailRequest(body)));
});
```

- [ ] `key:null` bedeutet keine explizite Client-Idempotenz, trotzdem durable Message; `parseEmailRequest(unknown): EmailRequest` führt Laufzeitvalidierung aus. Gemeinsamer Wrapper erzeugt genau eine Response und doppelt loggt nicht.
- [ ] Neue Next-Routeparams nach installierter Anleitung awaiten. Default Node-Runtime, `no-store` bei allen privaten Antworten; kein Set-Cookie im APIvertrag. Keine pauschale CORS-Freigabe für browsergespeicherte Keys.
- [ ] `request_id` und stabile Errornamen dokumentieren: `validation_error,missing_api_key,restricted_api_key,not_found,quota_exceeded,rate_limited,invalid_idempotent_request,concurrent_idempotent_requests,send_outcome_unknown,batch_send_failed,connection_unavailable`. Provider-/SQLfehler in sichere `delivery_error`/`service_unavailable` übersetzen.
- [ ] Full-Key nur eigene Ressourcen; einzelne Suppression-Adresse sicher URL-dekodieren/normalisieren, keine internen Pfad-Traversals. Für Scheduleänderungen erwartete Revision als `If-Match` übertragen; fehlende Revision 428, veraltete 409.
- [ ] OpenAPI 3.1 mit Request-/Responsebeispielen, Pagination, Raten-/Größenlimits, 202-Batch und Signature-Headers erstellen. Scalar lokal/versioniert einbinden; Remote-CDN-„latest“ vermeiden. „Try it“ wird erst bei ausdrücklich eingegebenem Testkey aktiv, keine Secrets serverseitig in Docs einsetzen.
- [ ] Tests über tatsächliche Routeexports ausführen. Testmatrix mit leerem/malformiertem JSON, Chunked Body über Limit, ungültigen Accept-/Content-Type-Kombinationen, falschen Keys und doppelten Sends.
- [ ] Run: neue API-/OpenAPItests; `npm run check -w @spitzli/auth`, gezielter ESLint über neue Files.
- [ ] Commit: `feat(mail): publish versioned email API and OpenAPI reference`.

**Abnahme:** Jede im Quickstart verwendbare APIoperation funktioniert gegen die isolierte Integrationstestumgebung.

## T15: Studio-Bridge, Mailnavigation, Nachrichten und Testversand

**Files:** Create `CORE/studio.ts`, `apps/admin/src/lib/mail-product.ts`, `COMP/{mail-navigation,connection-select,message-table,message-detail,message-preview,test-send-form,schedule-controls}.tsx`, `UI/layout.tsx`, `UI/emails/page.tsx`, `UI/emails/[id]/page.tsx`. Modify `apps/auth/src/lib/studio-api.ts`, `apps/admin/src/lib/{studio-contracts,studio-errors}.ts`, `apps/admin/src/components/navigation.tsx`, `UI/page.tsx`. Test `TEST/mail-product-studio.test.ts` und `apps/admin/tests/mail-preview.test.ts`.

**Interfaces:** `dispatchMailStudio(operation,args,headers): Promise<unknown>` akzeptiert nur die unten registrierten Formen; Adminwrapper verwendet vorhandenes `studioCall`. Neue Operationen: `mailConnections,mailOverview,mailMessages,mailMessage,mailContent,mailTestSend,mailReschedule,mailCancel`.

- [ ] Operationen mit genauen Typed-Argumenten registrieren; Customer-/Connection-IDs vom Backend gegen echte SSO-Session prüfen. Vorhandene 128-KiB-Inbound- und 2-MB-Outbound-Grenzen beibehalten. `mailContent` liefert einzeln maximal 512 KiB; Listen enthalten keinen Body.
- [ ] Previewzugriff an realer Bridge testen, nicht nur per verstecktem Button:

```ts
test("tenant preview rejects a hidden test-send request", async () => {
  await withMailFixture(async f => {
    await enterTenantPreviewForFixture(f);
    const response = await f.request("/api/studio", studioFixtureRequest(f, {
      operation: "mailTestSend",
      args: [f.actorA.customerID, f.actorA.connectionID, {
        from: "hello@" + f.domainA.name, to: "delivered@test.mail.webdock.dev",
        idempotencyKey: "preview-attempt",
      }],
    }));
    assert.equal(response.status, 403);
    assert.equal(f.provider.calls.length, 0);
  });
});
```

- [ ] `enterTenantPreviewForFixture` und `studioFixtureRequest` benutzen die vorhandenen echten Fixture-/SSOwege aus `tenant-preview.test.ts` und `studio-api.test.ts`; keine erfundene Previewrolle in die HTTP-Daten schreiben.
- [ ] Overview und Verbindungsauswahl in bestehende Shell integrieren. Mailunterseiten über `MailNavigation`; Tenantnavigation behält einen Mail-Einstieg und keine duplizierten widersprüchlichen Tabs. Bestehende Seiten `keys` und `tracking` weiterhin erreichbar.
- [ ] Nachrichtenliste mit SQL-Suche/Filter/Cursor, Loading/Error/Empty-State. Detail zeigt eigene Message-ID, Status pro Empfänger, Domain/Connection, Timeline und berechtigte Bcc-Details. Reconciliation nur als explizite berechtigte Aktion; GET einer Seite sendet oder verändert nichts.
- [ ] HTMLvorschau mit `sandbox=""` und CSP ohne externe Netzwerkinhalte. Test-HTML mit Script, Form, Trackingpixel, Meta-Refresh, CSS-URL und externem Bild erzeugt keinen Script-/Netzwerkzugriff. Textversion immer verfügbar.
- [ ] Testsendform verwendet feste Testvorlage, gewählten verifizierten Sender und ausdrücklich eingegebenen Empfänger. Formular-Idempotenz-Key bleibt bei Timeout stabil. Unbekannter Ausgang führt zur bestehenden Message; kein Retry-/Success-Toaster ohne belegtes Ergebnis.
- [ ] Schedule-Controls verwenden Message-Revision/If-Match-äquivalenten Bridgeparameter; Konflikt zeigt aktuellen Stand. Zeiten mit Zone sichtbar. Dialogfokus, Tastatur, Escape und Fehleransage unterstützen.
- [ ] Run: neue Studio-/Previewtests, vorhandene `studio-api.test.ts`, `tenant-preview.test.ts`, Admin-SSO-/Locale-Tests; Auth/Admin-Typechecks.
- [ ] Browserprüfung ausschließlich gegen lokale Fixtures: T3 `preview_status` → bei Bedarf `preview_open` → Snapshot/Interaktionen. Desktop/Mobil, beide Themes, Mitglied/Admin/Operator/Preview, Login-Rückkehr und absichtlich fremde Message-URL.
- [ ] Commit: `feat(mail): integrate message history and test sending into Studio`.

**Abnahme:** Sichtbare Mailaktionen laufen durch dieselben Dienste wie die Kunden-API; die Kundenansicht bleibt schreibgeschützt.

## T16: Vollständige Ressourcen-Konsole, Quickstart und Seitensuche

**Files:** Create `UI/{domains,api-keys,analytics,webhooks,suppressions,logs,quickstart,settings}/page.tsx`, `UI/domains/[id]/page.tsx`, `UI/webhooks/[id]/page.tsx`, `UI/logs/[id]/page.tsx`; fachliche Forms/Listen in `COMP/{domain-dns,api-key-form,analytics-chart,webhook-form,webhook-deliveries,suppression-form,request-log,quickstart,page-search,connection-settings}.tsx`. Modify `UI/tracking/page.tsx`, `UI/keys/page.tsx`, `apps/admin/src/components/hosting/own-mail.tsx`, `CORE/studio.ts` und Bridge-DTOs. Test `apps/admin/tests/mail-console.test.ts`.

**Interfaces:** Bridgeoperationen `mailDomains,mailDomain,mailDomainCreate,mailDomainVerify,mailDomainRemove,mailKeys,mailKeyCreate,mailKeyRevoke,mailAnalytics,mailWebhooks,mailWebhook,mailWebhookCreate,mailWebhookUpdate,mailWebhookDelete,mailWebhookRotate,mailWebhookReplay,mailSuppressions,mailSuppressionSet,mailSuppressionRemove,mailLogs,mailLog,mailConnectionUpdate,mailReconcile`; alle benötigen Customer+Connection, bis auf rein tenantweite Suppressiondaten.

- [ ] DNS-Onboarding zeigt vollständigen Recordnamen, Typ, Soll-/Istwert, Copy-Feedback, Konfliktgrund und unabhängigen Ownership-/Providerstatus. Bestätigte Records einklappen. Keine CF-Schaltfläche, OAuthroute oder automatisches Schreiben.
- [ ] „API-Schlüssel“ zeigt eigene `wdm_`-Keys, Permission/Domainbindung/Last-used/Widerruf. „SMTP-Zugangsdaten“ zeigt den separaten vorhandenen Direktpfad mit verständlicher Reichweitenangabe: Diese Mails haben keine automatische Webdock-Nachrichtenhistorie.
- [ ] Connectionsettings bieten die in T04 definierten Mutationen mit aktuellem Revisionstoken. Readiness unterscheidet Provider verbunden, Sender bereit, Versandkey vorhanden und Callbacks nachgewiesen; ein gesetztes Secret allein ist nicht „vollständig eingerichtet“.
- [ ] Analytics mit 7/30/90, Domain/Verbindungsfilter, Simulatorschalter, Kennzahlen, Tageschart und barrierefreier Tabelle. Gleiche Zahlen wie T13-API; kein eigener Client-Statusrechner.
- [ ] Webhooks vollständig: URL+Events, Create/Disable/Enable/Delete, Secret einmalig, Rotation, Versuchshistorie mit Status/Antwortauszug und Replay. Beim Secretwechsel klarstellen, dass Pending-Retries sofort den neuen Key nutzen.
- [ ] Sperrliste mit Grund/Quelle/Datum, Suche/Filter, manueller Sperre und bestätigter lokaler Entsperrung. Bestehende Provider-Sperre separat anzeigen; nicht den Eindruck einer vollständigen Providerentsperrung erwecken.
- [ ] Request-Logs nur für Admin/Owner/Full-Key; Query/Status/Datum/Request-ID und redigierte Detailansicht. Berechtigungswechsel zwischen Liste und Detail muss Zugriff entziehen.
- [ ] Quickstart mit kopierbaren cURL-/fetch-Beispielen für Send, To/Cc/Bcc, Anhänge/CID, Idempotenz/Unknown, Batch/202-Polling, Schedule/Cancel, Simulator und Webhook-Verifikation. Nur Platzhalter `SPITZLI_MAIL_API_KEY`; Dokumentationsbeispiele gegen Fixture-API ausführen.

```sh
curl https://auth.webdock.dev/api/mail/v1/emails \
  -H "Authorization: Bearer $SPITZLI_MAIL_API_KEY" \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: order-42-receipt" \
  -d '{"from":"Shop <mail@example.com>","to":"delivered@test.mail.webdock.dev","subject":"Receipt","text":"Thank you."}'
```

- [ ] Codebeispieltests parsen die tatsächlichen Strings; alle referenzierten Felder/Routen müssen in OpenAPI stehen. Keyplatzhalter werden nicht beim SSR durch echte Tenantsecrets ersetzt.
- [ ] Seitensuche/Cmd-K aus explizit freigegebenen Seiteneinträgen aufbauen; vorhandenes `@spitzli/search` für diese kleine lokale Liste wiederverwenden. Keine Nachrichtentexte, fremden Tenants, Keys oder Logs in den Index aufnehmen.
- [ ] Labels/Errorcodes mit `@spitzli/i18n` übersetzen: `npm run extract -w @spitzli/i18n`, Kataloge bearbeiten, `npm run compile -w @spitzli/i18n`, `npm run catalog:check -w @spitzli/i18n`. Keine englischen Reststrings in deutscher Oberfläche.
- [ ] Browserprüfung über T3 mit 360/768/1280/1440 CSS-Pixeln, hellem/dunklem Theme, langen Domain-/Betreffwerten, leeren/gefüllten Listen, Ladefehlern, Keyboardnavigation und Copy-Actions. Keine Tests, die nur CSS-Klassen spiegeln.
- [ ] Run: Admin-Console-/i18n-Tests und beide Typechecks/Lints; Dokumentationsbeispiel-Vertragscheck.
- [ ] Commit: `feat(mail): complete domains analytics operations and developer console`.

**Abnahme:** Alle im Relaykit-Bericht beschriebenen Konsolebereiche sind integriert; die ausgeschlossene CF-Funktion erscheint nicht als unfertiger Produktplatzhalter.

## T17: Team-Parität über bestehende Tenant-Mitgliedschaften

**Files:** Modify `apps/auth/src/lib/{tenants,tenant-policy}.ts`, `apps/auth/tests/tenants.test.ts`, `apps/auth/tests/tenant-preview.test.ts`, bestehende Tenant-Mitgliederform unter `apps/admin/src/app/(console)/(portal)/tenants/[customerID]`. Create `UI/team/page.tsx` und `TEST/mail-product-team.test.ts`. Keine neue Identity-/Membershiptabelle.

**Interfaces:** Bestehendes `manageTenant(headers,customerID,input)` behält seine Signatur und erhält die präzise Rollenpolicy aus der Spec. `getTenant` bleibt die Quelle für Mitglieder/Einladungen. Initiale Owner-Zuordnung erfolgt über den bestehenden Operatorpfad mit protokollierter ausdrücklicher Auswahl.

- [ ] Regressionssuite für heutige Tenant-/Projektgrants zuerst laufen lassen. T17 erweitert gezielt Zugriffsrechte und wird unabhängig reviewed; keine pauschale Freigabe nativer Better-Auth-Organisation-Mutationen.
- [ ] Matrix testen: Member liest/sendet; Admin lädt Member/Admin ein und verwaltet Members; Owner verwaltet Admins/Owners; Operator bleibt separater Plattformstatus. Ein Admin darf sich weder selbst zum Owner machen noch einen Owner entfernen.
- [ ] Letzten aktiven, verifizierten Tenant-Owner unter Organization-Rowlock schützen. Zwei parallele Ownerdemotions dürfen nicht beide erfolgreich sein:

```sql
SELECT id FROM webdock_auth.organization WHERE id=$1 FOR UPDATE;
-- Innerhalb derselben Transaktion aktuelle Rollen/Userzustände lesen,
-- dann mindestens einen verbleibenden verifizierten aktiven Owner verlangen.
-- Erst danach UPDATE/DELETE von member und gegebenenfalls project_grant.
```

- [ ] Bestehende Tenants ohne Kundenowner nicht zufällig migrieren. UI weist auf die notwendige Operatorzuordnung hin; alte zulässige Adminaktionen bleiben möglich, ownerpflichtige Aktionen sind bis dahin gesperrt.
- [ ] Einladung nur an normalisierten passenden verifizierten Nutzer; accept/reject/expired/canceled/wrong-email testen. `sendAuthMail` weiterhin außerhalb des Mailprodukts, mit Test-Outbox. Fehler/Resend dürfen keine zweite aktive Einladung mit widersprüchlicher Rolle erzeugen.
- [ ] Mitgliederentfernung sperrt sofort Studio/Mailzugriff und passende Projektgrants. Bestehende Tenant-API-Keys/Schedules behalten bewusst ihre Tenantzugehörigkeit; UI bietet gezielten Keywiderruf, ohne sie als personenbezogene Tokens auszugeben.
- [ ] `UI/team` nutzt dieselben Daten/Aktionen wie der Tenantbereich; keine persönliche Neon-/Relaykit-Accountmigration. Letzter Owner bleibt auch bei Role-/Ban-Races geschützt; Operatorrecovery ist auditiert.
- [ ] Run: `tenants.test.ts`, `tenant-preview.test.ts`, `mail-product-team.test.ts`, Auth-/Studio-Revoke-Tests und SSO-Checks.
- [ ] Commit: `feat(tenants): add owner-governed team management for Mail parity`.

**Abnahme:** Team-Parität schwächt weder Plattformoperator-Grenzen noch Rechte anderer Webdock-Produkte.

## T18: Durable Jobticks, Retention, Kill-Switches und Betriebsübersicht

**Files:** Create `CORE/{jobs,retention,health}.ts`, `apps/auth/src/app/api/cron/mail/[job]/route.ts`, `apps/auth/scripts/mail-operator.ts`, `TEST/mail-product-jobs.test.ts`, `mail-product-retention.test.ts`. Modify `apps/auth/vercel.json`, `apps/auth/src/lib/platform.ts`, Operator-Mailsettings und `CORE/studio.ts`.

**Interfaces:** `runMailJob(job: "dispatch"|"events"|"webhooks"|"maintenance",deadline): Promise<JobReport>`; `pruneMailData(now): Promise<RetentionReport>`; `getMailHealth(actor): Promise<MailHealth>`. `JobReport`/`RetentionReport` enthalten Zähler/Dauern, keine Mailinhalte.

- [ ] Readiness/Health und Flags definieren: globales `mailGatewayEnabled`, getrennte `mailDispatchEnabled`/`mailWebhooksEnabled`, Connectionstatus. Bestehendes Plattform-Mail-Off sperrt neue Plattformmutationen; vorhandene BYOK-Policy bleibt Quelle für BYOK-Freigabe. Ein ausdrücklicher neuer Gateway-Kill-Switch stoppt **beide** Gatewayarten, ohne Direkt-SMTP zu behaupten oder Authmails zu stoppen.
- [ ] Cronhandler mit constant-time `CRON_SECRET`-Prüfung, erlaubten Jobnamen und Produktionsumgebungsprüfung; isolierte lokale Tests verwenden injizierten Jobkontext. Ein Preview darf nie durch passende Secretkopie produktive Jobs aktivieren.
- [ ] Jobbudget und Claimstrategie implementieren: je Tick höchstens 45 Sekunden und vier parallele Netzwerkanfragen, Dispatch/Event/Webhook getrennt. Tenantfaire Auswahl mit Round-Robin über bis zu 20 aktive Tenants pro Tick und höchstens fünf Dispatches pro Tenant; persistierter Cursor verhindert Starvation bei mehr als 20 Tenants.
- [ ] Vercelkonfiguration bei bestätigtem Pro/Enterprise:

```json
{
  "crons": [
    { "path": "/api/cron/storage-usage", "schedule": "0 * * * *" },
    { "path": "/api/cron/mail/dispatch", "schedule": "* * * * *" },
    { "path": "/api/cron/mail/events", "schedule": "* * * * *" },
    { "path": "/api/cron/mail/webhooks", "schedule": "* * * * *" },
    { "path": "/api/cron/mail/maintenance", "schedule": "17 * * * *" }
  ]
}
```

- [ ] Vorhandene `regions/git`-Konfiguration erhalten. Falls T01 stattdessen K8s-Ticks erfordert, genau diese Handler aus einem `concurrencyPolicy: Forbid`-CronJob aufrufen; keine zweite Dispatchimplementation und nicht beide Scheduler einschalten.
- [ ] Arbeitsclaims mit Token fencen; `dispatching` nach Timeout zu Unknown-Reconciliation, nicht Retry. Webhookleases dürfen auslaufen und erneut zustellen. Testausführung mit zwei konkurrierenden Ticks, Clockvorlauf und Crash nach Side Effect.
- [ ] Retentiontabelle der Spec vollständig umsetzen. Grenztests:

```ts
test("cleanup never releases an unresolved send for a retry", async () => {
  await withMailFixture(async f => {
    f.provider.reply = { kind: "unknown", code: "lost_response" };
    await f.send(validEmail(), "retain-unknown");
    f.setNow(new Date(f.now.getTime() + 8 * 86400_000));
    await pruneMailData(f.now);
    await f.send(validEmail(), "retain-unknown");
    assert.equal(f.provider.calls.length, 1);
    const rows = await f.db.query(
      "SELECT status FROM webdock_auth.mail_idempotency WHERE key=$1",
      ["retain-unknown"],
    );
    assert.equal(rows.rowCount, 1);
  });
});
```

- [ ] Unknown-Payload nach sieben Tagen löschen, minimalen Outcome-/Idempotenznachweis erhalten. Content- und Attachmentbereinigung getrennt; aktive Schedules/Outbox/Leases nicht durch Altersfilter zerstören. `ON DELETE CASCADE` darf keine noch benötigte Delivery löschen.
- [ ] Health: letzter erfolgreicher Tick, ältester fälliger Schedule, älteste Inbox/Outbox, Unknown-Anzahl, Callbackquelle zuletzt gesehen, Credentialstatus, Quotenblockaden. Alarmzustand bei Tick >3 Minuten, Due-Lag >5 Minuten, Inbox-Lag >2 Minuten, Unknown >5 Minuten. Ein inaktives Konto ohne Sends braucht keinen falschen „Callback ausgefallen“-Alarm.
- [ ] `mail-operator.ts` bietet rein lesende Reports sowie ausdrücklich benannte reconcile/pause/resume-Aktionen; raw Retry unbekannter Sends existiert nicht. Logs nur IDs, Phasen, Fehlerklasse und Dauer.
- [ ] Lasttestprofil: 100 Tenants, 100.000 Message-Metadaten und 1.000.000 Events, 20 API-Requests/Sekunde, 120 angenommene Empfänger/Minute als initiales Pilotziel. Falsche Darstellung einer Garantie vermeiden: p95 DB-Lesezeit <500 ms, API-Overhead ohne Provider <300 ms, Due-Lag <120 Sekunden werden gemessen.
- [ ] Run: Jobs-/Retentiontests, `storage-cron.test.ts` und komplette Mailtestsuite.
- [ ] Commit: `feat(mail): operate durable mail jobs retention and health controls`.

**Abnahme:** Ein Prozessabbruch verliert keine angenommene Arbeit. Betriebsgrenzen sind messbar und stimmen mit dem tatsächlich verwendeten Scheduler überein.

## T19: Bestandsmigration und optionaler Relaykit-Datenimport

**Files:** Create `apps/auth/scripts/{mail-backfill,mail-import-relaykit}.ts`, `CORE/import.ts`, `TEST/mail-product-migration.test.ts`, `docs/architecture/mail-migration.md`. Modify `docs/architecture/platform-mail.md` mit dem finalen Daten-/Credentialmodell.

**Interfaces:** `backfillMailConnections({dryRun}): Promise<MigrationReport>`; `importRelaykit({sourceReadOnlyURL,mappingFile,dryRun}): Promise<MigrationReport>`. Report enthält nur Counts, Mapping-/Fehler-IDs und Prüfsummen, keine Bodies/Secrets.

- [ ] Webdock-Backfill zuerst: je Plattform-/BYOK-Quelle genau ein lokales Binding, Domains/Tracking zuordnen, Originaltabellen erhalten. Wiederholung erzeugt weder doppelte Connections noch Providerrequests.
- [ ] Migration konkurrierender Alt-/Neuschreibpfade kontrollieren: neue Gatewaywrites bis zum Backfill aus; nach Backfill alle vorhandenen Mutationen auf gemeinsamen Connectiondienst umstellen. Credentialrotation zwischen Inventory und Commit über Revision erkennen, nicht überschreiben.
- [ ] Produktive Readiness separat: keine internen Sendekeys oder Providerdomains in SQLmigration anlegen. Diese folgen pro bestätigter Connection über T04.
- [ ] Relaykit-Importer verlangt explizite Mappingdatei:

```json
{
  "version": 1,
  "accounts": [
    {
      "legacyAccountID": "source-account-id",
      "customerID": "confirmed-webdock-customer-id",
      "historicalConnectionID": "confirmed-original-provider-connection-id"
    }
  ]
}
```

- [ ] Mapping darf keine globale Shared-Account-Historie übernehmen. `legacy user_id` ist Ressourcenaccount, nicht automatisch aktueller Nutzer; Organization-Account-Zuordnung aus der Quelle berücksichtigen. Unmapped Rows zählen und ablehnen, nicht einem Defaultkunden zuweisen.
- [ ] Mehrere Relaykit-Ressourcenaccounts desselben Provideraccounts auf unterschiedliche Webdock-Tenants werden vom Standardimport mit `shared_provider_split_required` abgelehnt. Nicht die Unique-Accountbindung abschwächen. Für diesen gesonderten Migrationsfall braucht es eine bestätigte Historienaufteilung; bis dahin alte Schedules im einen Altexecutor belassen oder ausdrücklich stornieren.
- [ ] Dry-run validiert Domains, Provideraccount, IDs, Contentgrößen, Secret-Entschlüsselung/Neuverschlüsselung und Statuskompatibilität. Bereits eingetretene Retentionfristen kennzeichnen; Import setzt sie nicht heimlich zurück.
- [ ] Atomare Importbatches mit `mail_import_map`, Checksumme und Cursor. Zweiter Lauf mit identischer Quelle hat identische Counts. Bodies bleiben im geschützten DBpfad; Exportdateien nicht im Repo. Große/schädliche Altinhalte werden gemeldet und nicht in den HTML-Renderer eingeschleust.
- [ ] Bestehende `rk_`-Keys standardmäßig neu ausgeben; alte APIbasis wird nicht ungefragt umgebogen. Optionaler read-only Historienimport benötigt keine Keyübernahme. Domainownership durch neue TXT-Prüfung oder bewusst dokumentierte zeitlich begrenzte Legacyproof-Verifikation übernehmen.
- [ ] Aktive Schedules/Webhooks: alten Executor stoppen, Annahme einfrieren, Delta importieren, Ausführungsownership setzen, neuen Executor starten. Bereits `queued/unknown` gewesene Einträge bleiben Reconciliationfälle. Keine automatische Umsendung über ein neues Unterkonto.
- [ ] Callback-Cutover nur nach Inventory des aktuellen Consumers. Bei gemeinsamem Parentcallback vor Umschalten durable Fan-out einrichten oder den Altconsumer im Cutoverfenster kontrolliert übernehmen; beide Routen müssen Message-IDs abgleichen. Rückweg zum alten Consumer dokumentieren.
- [ ] Tests mit Relaykit-Fixturnachrichten: eigener/fremder Account, übergroße MID, Historie+Pending+Unknown, abgelaufener Log, Domainbindung, verschlüsseltes Webhooksecret, doppelter Import und Crash nach einem Batch.
- [ ] Run: `tests/mail-product-migration.test.ts` und Schema-/Callbacktests; Dry-runreport in `docs/verification` ohne vertrauliche Daten ablegen.
- [ ] Commit: `feat(mail): add resumable legacy backfill and Relaykit import tooling`.

**Abnahme:** Bestands-SMTP bleibt erreichbar; historischer Provideraccount und ausführender Worker sind nach Migration eindeutig.

## T20: Gesamtabnahme, Freigabe und Rollback

**Files:** Create `docs/architecture/mail-operations.md` und `docs/verification/webdock-mail-relaykit-acceptance.md`; ergänze `apps/auth/README.md`, `apps/admin/README.md`, öffentliche Datenschutz-/Aufbewahrungstexte an ihren vorhandenen Stellen. Keine zweite Deploypipeline erzeugen.

**Interfaces:** Verbindliche Abnahme F01–F21 mit Commit, Environment, Zeit, Check und Ergebnis. Der Runbookteil enthält exakte Freigabe-/Rückrollschritte für die in T01 gewählte Plattform.

- [ ] Gesamttest auf dem finalen Branch durchführen:

```sh
npm run test:mail -w @spitzli/auth
npm run test -w @spitzli/auth
npm run test -w @spitzli/admin
npm run check -w @spitzli/auth
npm run check -w @spitzli/admin
npm run lint -w @spitzli/auth
npm run lint -w @spitzli/admin
npm run catalog:check -w @spitzli/i18n
npm run build -w @spitzli/auth
npm run build -w @spitzli/admin
```

- [ ] App-spezifische Test-/Buildenv vorher bestätigen; keine produktiven Providercredentials in Tests. Build führt keine Migration aus. Bestehende Auth-/Hosting-/Storagefehler getrennt erfassen, nicht die Mailprüfung grün deklarieren, wenn erforderliche Checks scheitern.
- [ ] Browserprüfung über T3-Preview gegen die fertig migrierte isolierte Umgebung: Domainsetup → Webdockkey → simulierte Mail → Detail/Analytics → Webhookversuch → Replay → Suppression → Schedule/Cancel → Teamwechsel. Jede Clientaktion muss den echten Backendzustand ändern, keine Fixture-only-UI.
- [ ] Security-/Race-Abnahme: zwei Tenants und zwei Connections je Tenant, fremde Resource-ID, revoked Key, Mitgliedsentzug, Tenantarchive, Preview, Doubleclick, paralleles Idempotenzrequest, Callback vor Sendantwort, Duplicateevent, gleichzeitiger Cron, DNS-Rebinding, malicious HTML, falsche 64-Bit-ID, Retention während aktivem Job.
- [ ] Provider-Testnachweise aus T01 schließen. Kontrollierte Live-Abnahme als gesondert autorisierten Schritt planen: genau benannte Testsender/-empfänger, kleine festgelegte Nachrichtenanzahl, eigene signaturprüfende Webhook-URL und zweite Testperson. Keine Kunden kontaktieren oder ihre Anwendungen umstellen, nur weil der Plan existiert.
- [ ] Live-Ergebnis unterscheidet angenommen, zugestellt und tatsächlich im eigenen Testpostfach angekommen. Callback mit eigener Message referenzieren; Konsole und Analytics müssen dieselbe Aussage liefern. Ein getesteter Öffnungs-/Klickevent ist keine Zustellbarkeitsgarantie.
- [ ] Teamabnahme: Einladung über Authmail, passende verifizierte Annahme, Member/Admin/Ownergrenzen und sofortiger Zugriffsentzug; keine realen Personen ohne ausdrücklich benannten Testauftrag einladen.
- [ ] Vercel CLI für die spätere Ausführung installieren: `npm i -g vercel` ist empfohlen. Danach Projektzuordnung/Environment prüfen und `vercel env pull`/`vercel deploy`/`vercel logs` gemäß tatsächlichem Deployment verwenden; Envwerte nie im Report ausgeben.
- [ ] Releasefolge durchführen: Restorefähigkeit → additive Migration → Backend/Gates aus → vorhandene Connections backfillen → Pilotkey/Callback → getrennte Worker → Studio/API-Pilot → begrenzte Tenantkohorte → allgemeine Freigabe. Direkte SMTPkeys erst nach bewusstem Anwendungscutover widerrufen.
- [ ] Mindestens 24 Stunden Pilotbetrieb einschließlich eines echten fälligen Schedules, eines Retry-Zyklus, manueller Pause/Fortsetzung und Maintenance beobachten. Für längere Retryfristen zusätzlich beschleunigte Testuhr verwenden, nicht über zwölf Stunden auf Feedback im Chat blockieren.
- [ ] Rollbackprobe: neue Annahme stoppen, Dispatch/Webhooks pausieren, Inbox weiter persistent annehmen, keine Unknowns erneut senden, additive Tabellen/Importmaps erhalten. Letzte tenant-/rolekompatible Appversion wiederherstellen; keine Downgradeversion mit alten Authgrenzen.
- [ ] Fertigmeldung nur mit Abnahmematrix, finalen Testergebnissen, konkreten verbleibenden Einschränkungen und Deploymentstand. Falls ein PR erstellt/bearbeitet wird, T3-`link_pull_request` sofort verwenden und vor Abschluss gegen `list_thread_pull_requests` prüfen.
- [ ] Commit: `docs(mail): record end-to-end acceptance and production runbook`.

**Abnahme:** Alle beauftragten Funktionen einschließlich Betrieb sind nachgewiesen. Nicht durchgeführte Live-/Import-/Cutover-Schritte bleiben offen ausgewiesen; sie werden nicht aus bestandenen Unit-Tests abgeleitet.

## 4. Verbindliche Test- und Abnahmematrix

| Bereich | Minimaler Nachweis | Task |
| --- | --- | --- |
| Tenant-/Connection-Isolation | Gleiche MID, verschiedene Accounts; falsche IDs; aktuelle Membership | T02–T04, T11 |
| API-Key-Scope | Sending/Full, Domain-Tombstone, Revocation, keine Providerrechte | T03, T05, T14 |
| Domains | Ownership getrennt von Providerflags, SPF-Konflikt, strenge DMARC bleibt | T05, T16 |
| Kein CF-Scope | Keine CF-OAuth-/DNS-Schreibrouten, manuelle DNS-Anleitung funktionsfähig | T05, T16 |
| Einzelversand | Durable ID, richtige Credentials, definitive Ablehnung/Unknown | T08 |
| Anhänge/CID | Base64-/Größencheck, keine URL, nach Annahme Inhalt gelöscht | T08, T18 |
| Doppelversand | Parallelretry, Antwortverlust, DB-Ausfall nach Handoff | T07–T10 |
| Scheduling | Timezone, 30 Tage, Cancelrace, Domain-/Tenantentzug | T09 |
| Batch | Gesamtpreflight, Teilfehler, 100 langsame Items, 202/Polling | T10 |
| Simulator | Drei Outcomes, keine externen Sends/echten Suppressions/Quote | T08 |
| Callbacks | Flat/Array, 64-Bit-IDs, Aliases, Quelle, Replay, falsche Reihenfolge | T11 |
| Sperren | Bounce/Complaint/Unsubscribe, Cc/Bcc, lokale Entsperrung | T06, T08 |
| Analytics | Einheitliche lokale Quelle, klare Denominatoren, Multi-Recipient | T13 |
| Webhooks | Rawbody-HMAC, Secretrotation, Retry/Replay, SSRF, Leasecrash | T12 |
| Logs | Keine Secrets/Body/Bcc/Base64, Limits, 30 Tage, fremder Log | T07, T18 |
| UI | Echte Backendflows, sichere Vorschau, vier Breiten/zwei Themes/i18n | T15–T16 |
| Team | Zweiter Nutzer, last-owner-Race, Authmail, Grantrevocation | T17 |
| Betrieb | Leases, feste Budgets, faire Tenants, Restore/Kill-Switch/Rollback | T18, T20 |
| Migration | Dry-run, zweimaliger Import, Shared-Account-Isolation, Single-Executor | T19 |
| Dokumentation | Alle Beispiele gegen Fixture-API, OpenAPI deckt jede Route ab | T14, T16 |

## 5. Entscheidungen und verbleibende Nachweise

Der Plan enthält keine funktionalen Platzhalter. Folgende Entscheidungen sind als vorgeschlagene Defaults konkret getroffen und bei Umsetzung zu beachten: integrierte Auth-API statt zusätzlichem Dienst; beide Verbindungsarten; neue `wdm_`-Keys; minütliche PostgreSQL-Jobs; 30/90-Tage-Retention; 4-MB-JSONgrenze; lokale Empfängerbudgets; asynchron abschließbare große Batches; Ownerpolicy auf bestehender Tenantidentität.

Technische Nachweise, die Quellcode allein nicht liefern kann, stehen in T01/Spec Abschnitt 12: tatsächlicher BYOK-Accountnachweis, Callbackgranularität, Providerreferenz, Verbrauchszählung, Minuten-Scheduler, isolierte Previewenv und Restore. Kein Task darf daraus eine unbestätigte Infrastrukturbehauptung machen.

Der bestehende direkte SMTP-/Provider-API-Pfad bleibt als kompatibler Zugang erhalten. Vollständige Webdock-Historie, Webdock-Sperrlisten und lokale Quoten gelten für Sends durch die neue API. Ein eigener SMTP-Gateway wäre ein zusätzlicher Auftrag.

Cloudflare kann später als eigenständige Erweiterung geplant werden; dieser Plan enthält dafür weder Implementationstask noch Secret, Schema, UIplatzhalter oder Freigabebedingung.

## 6. Ausführung und Handoff

Empfehlung: in den fünf Etappen implementieren, nach jedem Task gezielter Test und Review, nach Etappen B/C/E eine Prüfung der gesamten verbundenen Versandstrecke. API-/DB-/Eventverträge zuerst festhalten, bevor UIarbeit parallelisiert wird.

Dieser Plan und die Spec sind das prüfbare Ergebnis des Planungsauftrags. Umsetzung beginnt erst mit einem entsprechenden Folgeauftrag. Es wurde hier kein Produktcode geschrieben, kein Providerkonto angelegt, kein DNS geändert und keine Nachricht versendet.
