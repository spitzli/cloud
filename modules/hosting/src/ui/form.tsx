"use client";
import * as React from "react";
import type { ComponentProps } from "react";
import { hostingUIScope } from "./scope";
import { useActionState, useState } from "react";
import Link from "next/link";
import { useUnsavedChanges } from "@spitzli/console-kit/use-unsaved-changes";
import { useI18n } from "@spitzli/i18n/react";
import type { HostingFormState } from "../ui-types";
import type { HostingCommand } from "@spitzli/hosting-contracts";
import {
  HostingSaveContext,
  HostingPendingContext,
  HostingDirtyContext,
} from "./form-state";
import { useHostingActions } from "./actions";
export { HostingSaveContext, HostingPendingContext } from "./form-state";
function HostingFormContent({
  command,
  children,
  label = "Save hosting settings",
  preserveRevision = false,
  disabled = false,
}: {
  command: HostingCommand;
  children?: React.ReactNode;
  label?: string;
  preserveRevision?: boolean;
  disabled?: boolean;
}) {
  const { saveHosting, displayEnrollment } = useHostingActions();

  const [initialCommand, setInitialCommand] = useState(command);
  const [dirty, setDirty] = useState(false);
  const { t, error } = useI18n();
  useUnsavedChanges(dirty);
  const [state, action, pending] = useActionState(
    async (previous: HostingFormState, formData: FormData) => {
      const result = await saveHosting(previous, formData);
      if (result.message) setDirty(false);
      const idempotencyKey = result.message ? crypto.randomUUID() : undefined;
      setInitialCommand((current) => ({
        ...current,
        ...("revision" in current && result.revision !== undefined
          ? { revision: result.revision }
          : {}),
        ...(idempotencyKey &&
        (current.action === "apps.create" || current.action === "apps.update")
          ? { idempotencyKey }
          : {}),
      }));
      return result;
    },
    {},
  );
  return (
    <HostingDirtyContext.Provider value={() => setDirty(true)}>
      <HostingSaveContext.Provider value={state}>
        <HostingPendingContext.Provider value={pending}>
          <form
            action={action}
            className="hosting-form"
            onChange={() => setDirty(true)}
          >
            <input
              type="hidden"
              name="command"
              value={JSON.stringify(
                preserveRevision ? initialCommand : command,
              )}
            />
            {children}
            {state.error && <p role="alert">{error(state.error)}</p>}
            {state.message && <p role="status">{t(state.message)}</p>}
            {state.operationPath && (
              <Link className="button secondary" href={state.operationPath}>
                {t("View progress")}
              </Link>
            )}
            {state.clusterPath && (
              <Link className="button secondary" href={state.clusterPath}>
                {t("Connect cluster agent")}
              </Link>
            )}
            {state.appPath && (
              <Link className="button secondary" href={state.appPath}>
                {t("Open application")}
              </Link>
            )}
            {state.setupPath && (
              <Link className="button secondary" href={state.setupPath}>
                {t("Open cluster enrollment")}
              </Link>
            )}
            <button className="button" disabled={pending || disabled}>
              {pending ? t("Saving…") : t(label)}
            </button>
          </form>
        </HostingPendingContext.Provider>
      </HostingSaveContext.Provider>
    </HostingDirtyContext.Provider>
  );
}
function EnrollmentFormState({ enrollmentID }: { enrollmentID: string }) {
  const { saveHosting, displayEnrollment } = useHostingActions();

  const { t, error } = useI18n();
  const [state, action, pending] = useActionState(
    displayEnrollment.bind(null, enrollmentID),
    {},
  );
  return (
    <form action={action}>
      <p>
        {t(
          "The token expires after five minutes and can connect exactly one agent. Store agent credentials privately on your node.",
        )}
      </p>
      {state.error && <p role="alert">{error(state.error)}</p>}
      {state.token ? (
        <section aria-live="polite">
          <p>
            {t("Save this enrollment token now. It is displayed only once.")}
          </p>
          <pre style={{ overflowWrap: "anywhere", whiteSpace: "pre-wrap" }}>
            {state.token}
          </pre>
          <p>
            {t("Expires at")}: {state.expiresAt}
          </p>
        </section>
      ) : (
        <button className="button" disabled={pending}>
          {t("Show enrollment token once")}
        </button>
      )}
    </form>
  );
}

export function HostingForm(props: ComponentProps<typeof HostingFormContent>) {
  return <HostingFormContent key={hostingUIScope(props)} {...props} />;
}
export function EnrollmentForm(
  props: ComponentProps<typeof EnrollmentFormState>,
) {
  return <EnrollmentFormState key={hostingUIScope(props)} {...props} />;
}
