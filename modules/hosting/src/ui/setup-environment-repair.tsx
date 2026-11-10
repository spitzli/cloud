"use client";
import * as React from "react";
import type { ComponentProps } from "react";
import { hostingUIScope } from "./scope";
import { useActionState, useMemo, useState } from "react";
import { useI18n } from "@spitzli/i18n/react";
import type { SetupView } from "@spitzli/hosting-contracts";
import { useUnsavedChanges } from "@spitzli/console-kit/use-unsaved-changes";
import { ApplicationEnvironment } from "./environment";
import {
  HostingDirtyContext,
  HostingPendingContext,
  HostingSaveContext,
} from "./form-state";
import { useHostingActions } from "./actions";
function SetupEnvironmentRepairState({
  view,
  requestKey,
}: {
  view: SetupView;
  requestKey: string;
}) {
  const { saveSetup } = useHostingActions();

  const { t, error } = useI18n();
  const [dirty, setDirty] = useState(false),
    [key, setKey] = useState(requestKey);
  const [state, action, pending] = useActionState(
    async (previous: Parameters<typeof saveSetup>[0], form: FormData) => {
      const result = await saveSetup(previous, form);
      if (!result.error) {
        setDirty(false);
        setKey(crypto.randomUUID());
      }
      return result;
    },
    {},
  );
  useUnsavedChanges(dirty);
  const saved = state.view ?? view,
    result = useMemo(
      () => (state.view && !state.error ? { message: "Saved" } : {}),
      [state],
    );
  return (
    <details className="workspace-disclosure">
      <summary>{t("Correct draft variables")}</summary>
      <HostingSaveContext.Provider value={result}>
        <HostingPendingContext.Provider value={pending}>
          <HostingDirtyContext.Provider value={() => setDirty(true)}>
            <form action={action}>
              <input type="hidden" name="projectID" value={view.projectID} />
              <input type="hidden" name="setupID" value={view.id} />
              <input type="hidden" name="revision" value={saved.revision} />
              <input
                type="hidden"
                name="configuration"
                value={JSON.stringify(view.configuration)}
              />
              <input type="hidden" name="idempotencyKey" value={key} />
              <input type="hidden" name="intent" value="save" />
              <p>
                {t(
                  "Remove conflicting manual variables, save, then retry setup. Provisioned services are kept.",
                )}
              </p>
              <ApplicationEnvironment names={saved.environmentNames} />
              {state.error && <p role="alert">{error(state.error)}</p>}
              {state.view && !state.error && <p role="status">{t("Saved")}</p>}
              <button className="button" disabled={pending}>
                {t("Save variables")}
              </button>
            </form>
          </HostingDirtyContext.Provider>
        </HostingPendingContext.Provider>
      </HostingSaveContext.Provider>
    </details>
  );
}

export function SetupEnvironmentRepair(
  props: ComponentProps<typeof SetupEnvironmentRepairState>,
) {
  return <SetupEnvironmentRepairState key={hostingUIScope(props)} {...props} />;
}
