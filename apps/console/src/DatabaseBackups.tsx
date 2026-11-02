import { useCallback, useEffect, useState } from "react";
import { type DatabaseView, type DatabaseBackupPolicyView } from "@webdock/hosting-contracts";
import { msgid } from "@webdock/i18n";
import { useI18n } from "@webdock/i18n/react";
import { useApi, useSession } from "./context";
import { sessionCacheScope } from "./session-cache";
import * as ui from "./ui";
import { useBackupRequest, type BackupRequestMemory } from "./database-backup-request";
import { useDatabaseBackupHistory } from "./use-database-backup-history";
import { backupObservationCurrent } from "./database-backup-observation";
import { BackupPolicyControls, BackupDestinationRegistration } from "./DatabaseBackupControls";

const states = {
  "not-configured": msgid("Backups are not configured"),
  "configuration-pending": msgid("Backup configuration pending observation"),
  configured: msgid("Backup configuration observed"),
  "suspension-pending": msgid("Backup suspension pending observation"),
  suspended: msgid("Backup schedule suspension observed"),
};
interface Props { memory: BackupRequestMemory; database: DatabaseView; operator: boolean; canWrite: boolean; readable: boolean; revalidate: () => Promise<boolean>; readDenied: () => void; refresh: () => void }
export function DatabaseBackups(props: Props) {
  const session = useSession(), api = useApi();
  const scope = JSON.stringify([sessionCacheScope(session), api.config.apiOrigin, props.database.projectID, props.database.id]);
  return <ScopedBackups key={scope} {...props} scope={scope} />;
}
function ScopedBackups({ memory, database: d, scope, operator, canWrite, readable, revalidate, readDenied, refresh }: Props & { scope: string }) {
  const session = useSession(), { t } = useI18n();
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { const timer = setInterval(() => { setNow(Date.now()); }, 1000); return () => { clearInterval(timer); }; }, []);
  const [unexpired, setUnexpired] = useState(() => Date.parse(session.session.expiresAt) > Date.now());
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>;
    const expire = () => { const left = Date.parse(session.session.expiresAt) - Date.now(); if (left <= 0) setUnexpired(false); else timer = setTimeout(expire, Math.min(2147483647, left)); };
    expire(); return () => { clearTimeout(timer); };
  }, [session.session.expiresAt]);
  const history = useDatabaseBackupHistory(memory, scope, d.id, d.kind === "dedicated", readable && unexpired, revalidate, readDenied);
  const writable = operator && canWrite && session.user.role === "operator" && !session.user.preview && unexpired && Boolean(history.data);
  const source = history.data?.policy;
  const request = useBackupRequest(memory, writable, session.session.expiresAt, () => { refresh(); history.observe(); }, target => Boolean(history.data && backupObservationCurrent(d.backup, history.data.policy, Date.now(), target) && d.backup.status === "configured"), source ? { id: source.id, revision: source.revision, revoked: source.revokedAt !== null } : null);
  const refreshIntent = useCallback((automatic = false) => {
    const token = memory.cell.beginRefresh(automatic);
    if (!token) return;
    void history.refresh(() => memory.cell.refreshCurrent(token)).then(valid => { if (memory.cell.finishRefresh(token, valid)) refresh(); });
  }, [memory.cell, history, refresh]);
  useEffect(() => { if (readable && memory.snapshot.needsReattach) refreshIntent(true); }, [readable, memory.snapshot.needsReattach, refreshIntent]);
  const current = Boolean(history.data && backupObservationCurrent(d.backup, history.data.policy, Math.max(now, history.receivedAt ?? now), request.policyTarget));
  const waiting = request.policyTarget !== null && !current;
  const policy = history.data?.policy;
  const status = current || d.backup.status === "not-configured" && !policy ? d.backup.status : policy?.revokedAt || d.backup.status.startsWith("susp") ? "suspension-pending" : "configuration-pending";
  return <section className="database-backups grid gap-4" aria-label={t("Backups")}>
    <h4 className={ui.sectionTitle}>{t("Backups")}</h4>
    <p role="status">{history.unavailable ? t("Backup observation unavailable. Refresh before making changes.") : waiting ? t("Backup request accepted. Waiting for agent observation.") : t(states[status])}</p>
    <p className={ui.notice}>{t("Public recovery is unavailable. No recovery window or RPO/RTO commitment is included.")} <code>production-qualification-required</code></p>
    <button className={ui.button()} disabled>{t("Restore database")}</button>
    <p className="text-sm text-muted">{t("Operator restore requires a separately reviewed recovery order. This Console flow is not available yet.")}</p>
    {d.kind === "shared" ? <p>{t("Shared PostgreSQL backup, restore and clone are unavailable.")}</p> : null}
    {history.unavailable ? <p>{t("Previous observations — not current confirmation")}</p> : <BackupObservations database={d} data={history.data} current={current} />}
    {history.data ? <BackupRequestFeedback request={request} /> : null}
    {request.state.status === "uncertain" && request.state.action !== "databases.backupDestinations.register" ? <button className={ui.button()} disabled={!writable} onClick={() => { request.retry(); }}>{t("Retry same request")}</button> : null}
    {d.kind === "dedicated" ? <button className={ui.button()} onClick={() => { refreshIntent(); }}>{t("Refresh backup status")}</button> : null}
    {history.unavailable && operator && session.user.role === "operator" && !session.user.preview ? <button className={ui.button()} disabled>{t("Back up now")}</button> : null}
    {operator && canWrite && session.user.role === "operator" && !session.user.preview && d.kind === "dedicated" && history.data ? <><BackupPolicyControls database={d} data={history.data} request={request} writable={writable} observed={current && status === "configured"} />{request.state.status !== "forbidden" ? <BackupDestinationRegistration request={request} writable={writable} /> : null}</> : null}
  </section>;
}

function BackupObservations({ database: d, data, current }: { database: DatabaseView; data: DatabaseBackupPolicyView | undefined; current: boolean }) {
  const { t, date } = useI18n(), observed = d.backup.observedControls;
  return <>
    {d.backup.lastSuccessAt ? <p>{t("Historical last completed backup")}: {date(d.backup.lastSuccessAt, { dateStyle: "medium", timeStyle: "short" })}</p> : null}
    {data?.policy ? <div className="grid gap-2 text-sm"><strong>{t("Desired controls")}</strong><p>{t("Daily schedule")}: <code>{data.policy.spec.schedule}</code> · {t(data.policy.spec.suspended ? "Suspended" : "Enabled")}</p><p>{t("WAL archiving")}: {t(data.policy.spec.walEnabled ? "Enabled" : "Disabled")}</p><p>{t("Retention deletion permission")}: {t(data.policy.spec.retentionDelete ? "Enabled" : "Disabled")}</p><p>{t("Retention days")}: {data.policy.spec.retentionDays}</p>{data.policy.revokedAt ? <p>{t(data.policy.revocationEffective && current ? "Backup grant revocation observed. WAL and retention permissions remain separate." : "Backup grant revocation is pending observation. WAL and retention permissions remain separate.")}</p> : null}</div> : null}
    {observed ? <div className="grid gap-2 text-sm"><strong>{t(current ? "Observed controls" : "Previous observations — not current confirmation")}</strong><p>{t("Daily schedule")}: <code>{observed.schedule}</code></p><p>{t("WAL archiving")}: {t(observed.walEnabled ? "Enabled" : "Disabled")}</p><p>{t("Retention deletion permission")}: {t(observed.retentionDelete ? "Enabled" : "Disabled")}</p><p>{t("Retention days")}: {observed.retentionDays}</p></div> : <p>{t("No agent observation is available.")}</p>}
    {data?.backups.length ? <div><h5 className="font-semibold">{t("Historical completed backups")}</h5><ul>{data.backups.map(b => <li key={b.id}>{date(b.completedAt, { dateStyle: "medium", timeStyle: "short" })} · {t("Restore point")}: <code>{b.id}</code></li>)}</ul><p className="text-sm text-muted">{t("The latest 100 recorded completions are shown. A completed backup alone does not establish a recovery window.")}</p></div> : null}
  </>;
}
function BackupRequestFeedback({ request }: { request: ReturnType<typeof useBackupRequest> }) {
  const { t } = useI18n(), receipt = request.state.receipt;
  return <>
    {request.state.key ? <p className="text-sm">{t("Request reference")}: <code>{request.state.key}</code></p> : null}
    {receipt ? <p role="status" className="text-sm">{"destinationID" in receipt ? <>{t("Registered destination ID")}: <code>{receipt.destinationID}</code></> : <>{t("Accepted operation")}: <code>{receipt.operationID}</code> · {t("Backup policy")}: <code>{receipt.policyID}</code></>}</p> : null}
    {request.state.error ? <p role="alert" className="text-sm text-danger">{t({ fields: msgid("Check the backup request fields. Refresh before trying again."), changed: msgid("Retry the original unchanged request. Re-enter the identical credentials for a destination registration."), conflict: msgid("Backup request conflicts with current state. Refresh before trying again."), forbidden: msgid("Backup authority is no longer available. Sign in again or request current operator access."), uncertain: msgid("The response was lost. The request may have been accepted. Retry only the identical request with its retained reference.") }[request.state.error])}</p> : null}
  </>;
}
