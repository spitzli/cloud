# Handover: S3, Managed Object Storage und dauerhafte Artefakte

Stand: 2026-10-25. Ausgangscommit: `819041524049117bc50e11dfd96a6a29407804b8`.

## Auftrag des Owners

Der Owner hat einen Wasabi-Account angelegt und möchte alle unten beschriebenen
S3-Anwendungsbereiche für Webdock haben. **Build-Cache ist ausdrücklich ausgeschlossen.**
Zusätzlich soll **Jenkins als TODO recherchiert und bewertet** werden. Das ist noch
keine Entscheidung, Jenkins einzusetzen oder Infrastruktur dafür aufzubauen.

Dieser Handover übergibt Zielumfang, vorhandene Grundlagen und Grenzen an einen neuen
Agenten. Er behauptet keine bereits implementierte S3-Anbindung und ersetzt nicht den
konkreten technischen Entwurf für noch offene Schnittstellen und Betriebsentscheidungen.
Keine erneute Grundsatzfrage stellen, ob diese Produkte gewünscht sind: Der Umfang ist
bestätigt. Fehlende Accountdetails gezielt klären und bis dahin lokal weiterarbeiten.

## Repository und bestehender Stand

- Worktree: `/home/newt/.t3/worktrees/webdock/t3code-rust-platform-rewrite`.
- Branch: `t3code/rust-platform-rewrite`; vor Beginn aktuellen HEAD und lokale Änderungen prüfen.
- `CLAUDE_HANDOVER.md` und `CODEX_HANDOVER.md` sind vorhandene geschützte, untracked
  Dateien: **nicht lesen, verändern, löschen oder committen**. Auch
  `.dev-local/demo-owner.json` nicht lesen. Diesen neuen Handover verwenden.
- Rust ist die aktive Plattform-API; Console ist React/Vite. Bun dient dem Tooling.
- Produktlogik gehört in Plugins beziehungsweise vorhandene Produktmodule. Bestehende
  Authentifizierung, Kunden-/Projektberechtigungen, Ressourcen, Angebote, Limits,
  Operationen und Audit verwenden; keine parallele Benutzer-/Queue-/Berechtigungsplattform.
- MinIO ist bereits in `deploy/dev/compose.yaml` und `Tiltfile` integriert.
  `tools/dev/minio.mjs` bereitet die lokale Konfiguration vor. Keine zweite lokale
  S3-Infrastruktur aufbauen und keine vorhandenen privaten Env-Dateien ausgeben.
- Native signierte Plugin-Installation, private Metadatenhaltung und generationstreue
  Updates/Rollbacks bestehen bereits. Unveränderliche Release-Provenienz und
  bestehende Runtime-/Agent-Fences erhalten.
- Sofortige Plugin-Jobs existieren. Wiederkehrende/systemeigene Jobs und
  installationsweite autonome Autorität sind noch ein eigener offener Bereich.
  Backup-Zeitpläne brauchen eine ausdrückliche dauerhafte Autorität; keine erfundenen
  Browser-Sessions oder Dummy-Kunden/Projekte zur Umgehung dieser Grenze.

Der letzte abgeschlossene Meilenstein ist Managed Mail Outbox:

- Commit `8190415` wurde signiert, Signatur und Remote-SHA wurden geprüft.
- 49 Gesamtchecks und zwölf echte lokale Core/Mail/SMTP-Abnahmefälle bestanden.
- Ein kanonischer Core-Send-Ledger behält Autorität, Budget, Leases und Receipts;
  Managed Mail hält verschlüsselte Payloads privat und führt den Versand aus.
- Lokale Testressourcen und das eigens gebaute Image-Tag wurden entfernt.
- Das ist keine Produktionsumstellung oder Freigabe für reale Provider-Zustellung.
- Die damalige PC-Shutdown-Anweisung wurde erledigt. **Nicht als Auftrag für diesen
  neuen Arbeitsabschnitt übernehmen und keine damaligen Zugangsdaten übernehmen.**

## Bestätigter Funktionsumfang

### 1. Gemeinsame S3-Grundlage und Storage-Produkt

S3-kompatible Speicherziele mit Wasabi als erstem externen Backend und MinIO für lokale
Tests. Providerdetails sollen die fachlichen Produkte nicht fest an Wasabi koppeln.
Die genaue Aufteilung in kleinen gemeinsamen Rust-Adapter und Storage-Plugin nach
Bestandsaufnahme festlegen; kein universelles neues Storage-Framework vorab bauen.

Benötigt werden Endpoint/Region/Bucket-Zuordnung, sicher gespeicherte Zugangsdaten,
TLS-Prüfung, Streaming, Multipart-Uploads, Integritätsprüfung und explizite Lifecycle-
und Löschregeln. Providerfähigkeiten tatsächlich prüfen; „S3-kompatibel“ bedeutet
nicht, dass alle IAM-, Versionierungs-, Lock- oder Lifecycle-APIs identisch sind.

Für Kunden soll daraus **Managed Object Storage** entstehen:

- Buckets beziehungsweise nachgewiesen isolierte Speicherressourcen pro Kunde,
  Projekt und Umgebung, mit nachvollziehbarer Eigentümerschaft.
- Eingeschränkte Zugänge, Rotation/Widerruf und geeignete Upload-/Downloadfreigaben.
  Kein gemeinsamer Wasabi-Root-Schlüssel für Kunden oder untrusted Workloads.
- Console, passende REST-/MCP-Funktionen, Nutzungsanzeige, Angebote/Abrechnung und
  transparente Limits über vorhandene Plattformmechanismen.
- Direkte App-Anbindung und kontrollierte Browser-Uploads/Downloads.
- Versionierung, Aufbewahrung, Trennen/Deaktivieren und endgültiges Löschen als
  verschiedene, explizite Operationen.

Speichermessung ist keine harte Quota. Bei direkten S3-Zugangsdaten kann eine bloße
Webdock-Inventur weitere Uploads nicht verhindern. Providerseitige Grenzen oder einen
kontrollierten Datenpfad bewusst entwerfen; sonst Limits ehrlich als weich ausweisen.
Multipart-Reservierungen, parallele Uploads, Überschreiben und abgebrochene Uploads
bei einer behaupteten harten Durchsetzung berücksichtigen.

### 2. Dauerhafte Build-Artefakte und Releases

`crates/build/src/artifact.rs` enthält heute einen privaten **workerlokalen**
`ArtifactStore`. Darauf aufbauen und dessen Integritäts-/Eigentumsprüfungen bewahren.

S3 soll abgeschlossene OCI-Archive, fertige Frontend-Ausgaben, erforderliche
Quell-Snapshots und Release-Nachweise wie Hashes, Signaturen und SBOMs dauerhaft halten.
Ein geprüfter Build soll von einem anderen Worker veröffentlicht und für erneute
Deployments beziehungsweise Rollbacks wiederverwendet werden können.

- Unveränderliche Referenzen, tatsächliche Byteanzahl und Inhalts-Hashes prüfen.
  Ein Multipart-ETag ist nicht pauschal der Inhalts-Hash.
- Job-/Releasezustand und Autorität bleiben im bestehenden dauerhaften Ledger;
  S3 hält die großen Bytes. Upload allein ist noch kein erfolgreicher Build/Release.
- Upload-Abschluss, verlorene Antworten, Neustarts und verwaiste Objekte sauber behandeln.
- Referenzierte Releases nicht durch pauschale TTL/GC löschen. Storage-Wechsel nicht
  als automatische Migration oder Löschung alter lokaler Artefakte behandeln.
- Isolierte Build-VMs erhalten keine dauerhaften Wasabi-Zugangsdaten. Vertrauenswürdige
  Worker übertragen autorisierte Inputs/Outputs, bestehende VM-Isolation bleibt erhalten.

**Nicht enthalten:** BuildKit-, Cargo-, npm-/Dependency-, Compiler- oder andere
Build-Caches. Dauerhafte Release-Artefakte sind ausdrücklich enthalten.

### 3. Backup- und Restore-Service

Zuerst dediziertes PostgreSQL: konsistente Basissicherungen, WAL-Archivierung und
Wiederherstellung zu einem Zeitpunkt. Vorhandene CNPG-/Agent-/Produktgrenzen prüfen
und passende aktuelle Backup-Komponenten samt Provenienz auswählen.

Danach Mail- und App-Daten einschließlich der jeweils notwendigen Metadaten,
Konfiguration und gesondert abgesicherten Schlüsselwiederherstellung.

- Zeitpläne, Aufbewahrung, Fehlerzustände, Benachrichtigung und beobachteter Backup-Status.
- Vollständiger Restore in ein frisches Ziel; Daten, Berechtigungen und Produktfunktion
  prüfen. Ein erfolgreicher Upload ist kein Restore-Nachweis.
- RPO/RTO und Wiederherstellungsfenster konkret festlegen und testen, bevor Angebote
  oder UI diese versprechen. Shared-PostgreSQL nicht stillschweigend mit derselben
  Restore-Semantik wie dedizierte Instanzen behandeln.
- Live-Datenbank-/Mailbox-Volumes nicht einfach während des Betriebs als ZIP sichern.
  Konsistenz und produktspezifische Wiederherstellung gehören zum Entwurf.
- Für Mail gehören Core-Ledger, private Plugin-Metadaten/Payloads und gegebenenfalls
  CE-Daten zusammen in das Recovery-Konzept. Restore darf bestätigte oder unsichere
  frühere Zustellungen nicht blind erneut senden. Bei unklarer historischer
  Versandlage geschlossen bleiben und rekonzilieren.
- Schlüssel, Passwörter, Verschlüsselungsformate und dauerhaft gespeicherte Identitäten
  erhalten; eine Sicherung ohne erreichbare Wiederherstellungsschlüssel ist unzureichend.

Object Lock/Unveränderlichkeit und getrennte Backup-Berechtigungen gezielt bewerten.
Keine irreversible Compliance-Aufbewahrung nebenbei aktivieren und nicht dieselben
Löschrechte für Builder, Kunden-Uploads und Backups vergeben.

### 4. CMS- und App-Dateien

Bilder, Dokumente und Uploads unabhängig vom Lebenszyklus einzelner Container halten.
Privater Zugriff und gegebenenfalls öffentliche Auslieferung/CDN sind eigene Entscheidungen.

Die bisherigen Blob-Zuordnungen sind noch keine fertig isolierte S3-Plattform. Eine
Umstellung vorhandener CMS-Daten braucht explizite Quell-/Ziel-Zuordnung, Byte-/Hash-
und Referenzprüfung, Erhalt bestehender URLs, finale Schreibsynchronisation und Rollback.
Keine bestehende Produktion oder alte Stores im Rahmen der Grundlagenarbeit ändern/löschen.

### 5. Geschützte Exporte und Downloads

Datenbankexporte, Projektarchive und große Berichte asynchron erzeugen, in S3 speichern
und dem berechtigten Kunden bereitstellen. Eigentum, Freigabedauer, Löschung und
Download-Autorität prüfen; Linkbesitz und aktuelle Mitgliedschaft sind nicht automatisch
dieselbe Autorität. Große Dateien sollen nicht durch normale JSON-API-Frames laufen.

### 6. Archivierte Logs und Diagnosepakete

Abgeschlossene Buildlogs sowie bereinigte Diagnose-/Supportpakete komprimiert ablegen,
mit begrenzter Aufbewahrung und mandantengeprüftem Abruf. Keine Secrets, Tokens,
Verbindungs-URLs mit Zugangsdaten oder Mail-Nachrichtenkörper in Diagnosearchive übernehmen.
Live-Tailing, Suche und Monitoring bleiben in geeigneten Diensten; S3 ist das Archiv.

### 7. Container-Registry mit S3-Backend

Eine authentifizierte OCI-Registry mit S3 als Speicherbackend integrieren. S3 ersetzt
nicht die Registry-API. Vorhandene Digest-/Manifestprüfungen, Publishing-Autorität,
private Registry-Zugänge und beobachtete Deployment-Readiness bewahren.

Aufbewahrung/GC muss Referenzen aktiver Releases und Rollback-Ziele beachten. Backend-
Kompatibilität, Ausfall-/Neustartverhalten und reale Push-/Pull-Vorgänge prüfen; kein
ungeprüftes Produktions-Registry-Migrationsprojekt daraus machen.

## Wasabi: bekannt und noch offen

Bestätigt ist nur, dass der Owner einen Account hat. Bucket, EU-Region, Tarif,
Berechtigungen, Endpoint und produktiver Status wurden noch nicht bestätigt.

- Region und bestehende Ressourcen zuerst klären; keine bereits vorhandenen Buckets
  automatisch übernehmen oder löschen. EU-Anforderungen gelten weiterhin.
- Zugangsdaten über den vorgesehenen privaten Secret-Mechanismus anfordern. Niemals
  in Chat, Handover, Git, Testausgabe oder Screenshots einfügen.
- Aktuelle Konditionen erneut prüfen. Für das recherchierte Pay-as-you-go-Modell gelten
  1 TB Mindestabrechnung und 90 Tage Mindestspeicherdauer; frühes Löschen kann weiter
  berechnet werden. Die Free-Egress-Regel ist an das Verhältnis von Downloads zu
  aktivem Speicher gebunden. Kein unbegrenztes CDN-/Transfer-Versprechen daraus ableiten.
- Ein verkäufliches Kundenprodukt braucht einen passenden nachgewiesenen Account-/
  Berechtigungs-/Vertragsaufbau; dessen Möglichkeiten nicht aus einem einfachen Account ableiten.
- Lokale MinIO-Nachweise und echte Wasabi-Kompatibilität getrennt ausweisen. Remote-
  Tests zunächst auf ausdrücklich eigene, begrenzte Testressourcen beschränken.

Aktuelle offizielle Ausgangsquellen, vor Implementierung auf Änderungen prüfen:

- Wasabi Konditionen: <https://wasabi.com/product-terms>
- Wasabi Preis-/Egress-Regeln: <https://wasabi.com/pricing/faq>
- Regionen/Endpoints: <https://docs.wasabi.com/docs/service-urls-for-wasabis-storage-regions>
- Object Lock: <https://docs.wasabi.com/docs/immutability-compliance-and-object-locking>
- Quoten: <https://docs.wasabi.com/docs/enabling-storage-quota-usage>
- Registry-S3-Treiber: <https://distribution.github.io/distribution/storage-drivers/s3/>

## Architekturgrenzen und Betrieb

- Object Storage ist kein POSIX-Dateisystem und kein Ersatz für Datenbank-/App-Volumes.
  Kein S3-Mount als Standard-PVC für PostgreSQL oder SQLite.
- Storage-Produkt, Build-/Releaseprodukt und Backup-Produkt teilen kleine notwendige
  Schnittstellen, behalten aber ihre fachlichen Zustände und Autorität.
- Kunden-, Projekt-, Umgebungs- und Zweckgrenzen auch am Datenzugriff durchsetzen.
  Ein gemeinsamer Prefix ohne belastbare Provider-Policy ist kein Isolationsnachweis.
- Laufende Ressourcen, Keys, Private Storage und Verschlüsselungs-Master erhalten.
  Neue native Migrationen nur vorwärts; bestehende Migrationen nicht umschreiben.
- Kein automatisch eingeschlossener Produktions-/Contabo-/Spitzli-Cutover, keine
  fremden Providerressourcen oder breit wirkenden Cleanup-/Prune-Aktionen.
- Stalwart bleibt Community Edition, eine isolierte Instanz je Kunde mit ausdrücklich
  aktiviertem Incoming. Outgoing-only, Storage oder Backups provisionieren keinen Mailserver.
- Lokale Fixture-Ressourcen müssen eigene nachgewiesene Eigentümerschaft, bounded
  Laufzeit, exakte Cleanup-Nachweise und private Diagnosehaltung haben. Vorhandene
  Dev-Dienste oder den gestoppten lokalen k3s nicht ungefragt ändern/starten.

## Sinnvolle Umsetzung in überprüfbaren Abschnitten

Dies ist eine vorgeschlagene Reihenfolge, keine Streichung späterer Anforderungen:

1. Vorhandene Storage-, Plugin-, Build-, Backup- und Abrechnungsschnittstellen lesen;
   daraus konkreten Entwurf und Implementierungsplan mit offenen Produktentscheidungen ableiten.
2. S3-Grundlage gegen vorhandenes MinIO; dann begrenzte echte Wasabi-Kompatibilität.
3. Dauerhafte Build-Artefakte als erster interner Nutzer, ohne Build-Cache.
4. PostgreSQL Backup/Restore einschließlich Wiederherstellungstest.
5. Kundenfähiger Managed Object Storage mit ehrlichen Berechtigungs-/Quota-Zusagen.
6. CMS/App-Dateien, geschützte Exporte und Log-/Diagnosearchive.
7. Mail-/App-Backups und Registry-Backend; benötigte gemeinsame Teile gegebenenfalls
   früher einordnen, ohne Autoritäts- oder Recovery-Grenzen zu überspringen.

Für jeden Abschnitt: reale End-to-End-Abnahme, falscher Mandant/Widerruf, unterbrochene
Uploads, verlorene Antworten, Neustarts, Integritätsfehler, Aufbewahrung und Cleanup
testen, soweit für den Abschnitt relevant. Bestehende Release-/Rollback-Funktion darf
keine referenzierten Artefakte verlieren. Eine Wiederherstellung tatsächlich durchführen.
Console schmal/breit prüfen, passende REST-/MCP-Parität herstellen und Erfolgs-/Limit-
Versprechen nur nach gemessenem Nachweis anzeigen.

## Jenkins: ausdrückliches TODO, kein Implementierungsauftrag

- Jenkins für Webdock recherchieren: Nutzen gegenüber vorhandenem Buildworker und
  externen CI-Anbindungen, Betrieb/Wartung, Plugin-Abhängigkeiten, Zugangskontrolle,
  isolierte beziehungsweise kurzlebige Runner und Mandantentrennung.
- Prüfen, wie Jenkins freigegebene Artefakte an Webdock übergeben könnte und wer dabei
  Commit-, Provenienz-, Signatur-, Freigabe- und Deployment-Autorität hält.
- Prüfen, ob S3-Artefaktspeicherung sinnvoll integrierbar ist, ohne eine zweite
  konkurrierende Release-Wahrheit oder dauerhafte Secrets in Buildjobs einzuführen.
- Ergebnis als begründete Entscheidung festhalten. Jenkins nicht automatisch
  installieren, öffentlich exponieren oder an Produktionszugänge anschließen.
- **Auch über Jenkins keinen ausgeschlossenen Build-Cache nachträglich einführen.**

## Einstiegspunkte zum Lesen

- `AGENTS.md` und die vom Owner bereitgestellten Projektregeln.
- `docs/plugin-ideas.md`, `TODO.md`.
- `crates/build/README.md`, `crates/build/src/{artifact,worker,publisher}.rs`.
- `crates/build/tests/{artifact,publisher,execution}.rs`.
- `docs/operations/native-plugin-installation.md` und die zugehörigen Runtime-/Host-Schnittstellen.
- `docs/operations/managed-postgresql.md`, `plugins/postgres`, `crates/databases`.
- `docs/architecture/plans-and-storage.md` als historische Bestands-/Migrationsinformation;
  ältere TypeScript-/Vercel-Wege nicht ungeprüft als aktive Rust-Zielarchitektur übernehmen.
- `docs/architecture/persistent-app-storage.md`, `crates/agent/src/storage`.
- `docs/architecture/mail-deployment-decision.md`, `docs/operations/native-mail-outbox.md`.
- `docs/verification/2026-10-25-native-mail-outbox.json`, `plugins/mail/src/{payload,executor,store}.rs`.
- `Tiltfile`, `deploy/dev/compose.yaml`, `tools/dev/minio.mjs`.

Nicht alles neu bauen: zuerst vorhandene Grenzen und Tests verstehen, fehlende Teile
gezielt ergänzen und die fachlichen Abschnitte einzeln reviewbar abschließen.
