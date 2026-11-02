import { useLayoutEffect, useRef, type SubmitEvent } from "react";
import { useI18n } from "@webdock/i18n/react";
import type { DatabaseBackupPolicyView, DatabaseView } from "@webdock/hosting-contracts";
import type { useBackupRequest } from "./database-backup-request";
import * as ui from "./ui";
import { formText } from "./form";
import { msgid } from "@webdock/i18n";

type Request = ReturnType<typeof useBackupRequest>;
interface Props { database: DatabaseView; data: DatabaseBackupPolicyView; request: Request; writable: boolean; observed: boolean }
const text = (form: FormData, key: string) => formText(form, key).trim();

export function BackupPolicyControls({ database: d, data, request, writable, observed }: Props) {
  const { t } = useI18n(), policy = data.policy, disabled = !writable || request.blocked;
  const submit = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault(); const form = new FormData(event.currentTarget);
    const common = { databaseID: d.id, revision: d.revision, schedule: text(form, "schedule"), retentionDays: Number(text(form, "retentionDays")), retentionDelete: form.get("retentionDelete") === "on" };
    void request.send(policy ? { ...common, action: "databases.backupPolicies.update", policyRevision: policy.revision, suspended: form.get("suspended") === "on", walEnabled: form.get("walEnabled") === "on" } : { ...common, action: "databases.backupPolicies.grant", destinationID: text(form, "destinationID") });
  };
  return <div className="grid gap-4">
    {!policy?.revokedAt ? <form key={policy?.revision ?? "grant"} aria-label={t("Backup policy settings")} onSubmit={submit}>
      <fieldset disabled={disabled} className="grid gap-3 rounded-lg border border-outline p-4 sm:grid-cols-2">
        <legend className="px-2 font-semibold">{t(policy ? "Change backup policy" : "Grant backup policy")}</legend>
        {!policy ? <label className={ui.field}>{t("Registered destination ID")}<input className={ui.input} name="destinationID" required pattern="[1-9][0-9]*" /></label> : null}
        <label className={ui.field}>{t("Daily schedule")}<input className={ui.input} name="schedule" required defaultValue={policy?.spec.schedule ?? "0 0 2 * * *"} pattern="0 [0-5]?[0-9] ([01]?[0-9]|2[0-3]) \* \* \*" /><span className="text-xs text-muted">{t("Six fields: 0 minute hour * * *. Provider schedule timezone applies.")}</span></label>
        <label className={ui.field}>{t("Retention days")}<input className={ui.input} type="number" name="retentionDays" min="1" max="3650" required defaultValue={policy?.spec.retentionDays ?? 7} /></label>
        {policy ? <><label><input type="checkbox" name="suspended" defaultChecked={policy.spec.suspended} /> {t("Suspend scheduled backups")}</label><label><input type="checkbox" name="walEnabled" defaultChecked={policy.spec.walEnabled} /> {t("Allow WAL archiving")}</label></> : <p>{t("A new grant enables the daily schedule and WAL archiving.")}</p>}
        <label><input type="checkbox" name="retentionDelete" defaultChecked={policy?.spec.retentionDelete ?? false} /> {t("Allow deletion under the retention policy")}</label>
        <p className="text-sm text-muted sm:col-span-2">{t("Schedule suspension, WAL archiving and retention deletion are independent. Stop the schedule before disabling WAL.")}</p>
        <button className={ui.button()}>{t(policy ? "Apply backup policy" : "Grant backup policy")}</button>
      </fieldset>
    </form> : null}
    {policy ? <div className="flex flex-wrap gap-3">
      <button className={ui.button()} disabled={disabled || !observed || policy.revokedAt !== null || policy.spec.suspended || !policy.spec.walEnabled || d.state !== "ready"} onClick={() => { void request.send({ action: "databases.backup", databaseID: d.id, revision: d.revision }); }}>{t("Back up now")}</button>
      <button className={ui.button("danger")} disabled={disabled || policy.revokedAt !== null} onClick={() => { if (window.confirm(t("Revoke the backup grant? Recurrence stops only after agent observation. WAL and retention deletion permissions are unchanged."))) void request.send({ action: "databases.backupPolicies.revoke", databaseID: d.id, revision: d.revision, policyRevision: policy.revision }); }}>{t("Revoke backup grant")}</button>
    </div> : null}
  </div>;
}
export function BackupDestinationRegistration({ request, writable }: { request: Request; writable: boolean }) {
  const { t } = useI18n();
  const nodeRef = useRef<HTMLFormElement>(null);
  const wipe = (form: HTMLFormElement) => { for (const input of form.querySelectorAll<HTMLInputElement>('[data-sensitive="true"]')) input.value = ""; };
  useLayoutEffect(() => { const form = nodeRef.current; return () => { if (form) wipe(form); }; }, []);
  const submit = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault(); const node = event.currentTarget;
    try {
      const form = new FormData(node);
      void request.send({ action: "databases.backupDestinations.register", endpointURL: text(form, "endpointURL"), region: text(form, "region"), bucket: text(form, "bucket"), keyRecoveryID: text(form, "keyRecoveryID"), writeCredentials: { accessKeyId: text(form, "writeAccessKey"), secretAccessKey: formText(form, "writeSecretKey"), readOnly: false }, recoveryCredentials: { accessKeyId: text(form, "recoveryAccessKey"), secretAccessKey: formText(form, "recoverySecretKey"), readOnly: true } });
    } finally {
      wipe(node);
      const endpoint = node.elements.namedItem("endpointURL");
      if (endpoint instanceof HTMLInputElement && !/^https:\/\/[a-zA-Z0-9.:-]+\/?$/.test(endpoint.value)) endpoint.value = "";
    }
  };
  const retry = request.state.status === "uncertain" && request.state.action === "databases.backupDestinations.register";
  return <details className="group/details rounded-lg border border-outline p-4"><summary className="cursor-pointer font-semibold">{t("Register backup destination")}</summary>
    <form ref={nodeRef} aria-label={t("Register backup destination")} onSubmit={submit} autoComplete="off" className="mt-4">
      <fieldset disabled={!writable || request.refreshPending || request.blocked && !retry} className="grid gap-3 sm:grid-cols-2">
        <p className="text-sm text-muted sm:col-span-2">{t("Provide separate writer and read-only recovery credentials. Credentials are sent once and cleared from this form. A recovery key reference does not establish key escrow.")}</p>
        {([ ["endpointURL", msgid("HTTPS S3 endpoint")], ["region", msgid("S3 region")], ["bucket", msgid("Private bucket")], ["keyRecoveryID", msgid("Retained recovery key reference")] ] as const).map(([name, label]) => <label className={ui.field} key={name}>{t(label)}<input className={ui.input} name={name} required /></label>)}
        {([ ["writeAccessKey", msgid("Writer access key")], ["writeSecretKey", msgid("Writer secret key")], ["recoveryAccessKey", msgid("Recovery access key")], ["recoverySecretKey", msgid("Recovery secret key")] ] as const).map(([name, label]) => <label className={ui.field} key={name}>{t(label)}<input className={ui.input} name={name} type="password" data-sensitive="true" autoComplete="off" required minLength={name.endsWith("SecretKey") ? 16 : 3} /></label>)}
        <button className={ui.button()}>{t(retry ? "Re-enter identical credentials and retry registration" : "Register destination")}</button>
      </fieldset>
    </form>
  </details>;
}
