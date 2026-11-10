"use client";
import * as React from "react";
import type { ComponentProps } from "react";
import { hostingUIScope } from "./scope";
import {
  useActionState,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useRouter } from "next/navigation";
import { useUnsavedChanges } from "@spitzli/console-kit/use-unsaved-changes";
import { msgid } from "@spitzli/i18n";
import { useI18n } from "@spitzli/i18n/react";
import type {
  SetupConfiguration,
  SetupOptions,
  SetupView,
  SetupInspection,
} from "@spitzli/hosting-contracts";
import type { SetupActionState } from "../ui-types";
import { canConfirmSetup, defaultSetupConfiguration } from "./setup-view";
import { SetupSourceFields } from "./setup-source-fields";
import { SetupServiceFields } from "./setup-service-fields";
import { SetupReview } from "./setup-review";
import { ApplicationEnvironment } from "./environment";
import {
  HostingSaveContext,
  HostingPendingContext,
  HostingDirtyContext,
} from "./form-state";
import { useHostingActions } from "./actions";
const stages = [
  msgid("Source"),
  msgid("Application"),
  msgid("Services"),
  msgid("Review"),
];
function SetupWizardState({
  options,
  initial,
  requestKey,
  initialStep = 0,
  mailSetup,
  initialInspection,
  initialError,
}: {
  options: SetupOptions;
  initial?: SetupView;
  requestKey: string;
  initialStep?: number;
  mailSetup?: ReactNode;
  initialInspection?: SetupInspection;
  initialError?: string;
}) {
  const { saveSetup, confirmSetup } = useHostingActions();

  const { t, error } = useI18n(),
    router = useRouter();
  const [saved, setSaved] = useState(initial),
    [configuration, setConfiguration] = useState<SetupConfiguration>(
      () =>
        initial?.configuration ??
        defaultSetupConfiguration(options.applications.map((a) => a.name)),
    );
  const [step, setStep] = useState(
      initial?.state === "awaiting-confirmation" ? 3 : initialStep,
    ),
    [key, setKey] = useState(requestKey),
    [accepted, setAccepted] = useState<string[]>([]),
    [dirty, setDirty] = useState(false),
    [connecting, setConnecting] = useState(false),
    [navigation, setNavigation] = useState<string | null>(null);
  const connectForm = useRef<HTMLFormElement>(null);
  useEffect(() => {
    if (connecting && saved?.id) connectForm.current?.requestSubmit();
  }, [connecting, saved?.id]);
  useUnsavedChanges(dirty);
  const [state, action, pending] = useActionState(
    async (previous: SetupActionState, form: FormData) => {
      const intent = String(form.get("intent") ?? "next");
      setNavigation(null);
      const result = await (intent === "deploy" ? confirmSetup : saveSetup)(
        previous,
        form,
      );
      if (result.view) {
        setSaved(result.view);
        setConfiguration(result.view.configuration);
        setKey(crypto.randomUUID());
        setDirty(false);
        setAccepted([]);
      }
      if (result.error) return result;
      if (result.view) {
        if (intent === "connect") setConnecting(true);
        else if (intent === "deploy")
          setNavigation(
            `/hosting/projects/${result.view.projectID}/setup/${result.view.id}`,
          );
        else {
          const next =
            intent === "review"
              ? 3
              : intent === "back"
                ? Math.max(0, step - 1)
                : intent === "inspect" || intent === "save"
                  ? step
                  : Math.min(3, step + 1);
          setStep(next);
          setNavigation(
            `/hosting/projects/${result.view.projectID}/setup/${result.view.id}?step=${next}${intent === "inspect" && !saved?.id ? "&inspect=1" : ""}`,
          );
        }
      }
      return result;
    },
    { inspection: initialInspection, error: initialError },
  );
  useEffect(() => {
    if (!pending && navigation) router.replace(navigation);
  }, [pending, navigation, router]);
  const environmentResult = useMemo(
    () => (state.view && !state.error ? { message: "Saved" } : {}),
    [state],
  );
  const update = (next: SetupConfiguration) => {
    setConfiguration(next);
    setDirty(true);
  };
  const spec = (patch: Partial<SetupConfiguration["spec"]>) =>
    update({ ...configuration, spec: { ...configuration.spec, ...patch } });
  const ready =
    !!saved?.preview &&
    saved.state === "awaiting-confirmation" &&
    canConfirmSetup({
      pending: pending || connecting || dirty,
      blockers: saved.preview.blockers,
      requiredOfferIDs: saved.preview.offers.map((o) => o.id),
      acceptedOfferIDs: accepted,
    });
  return (
    <div className="setup-workspace">
      <ol className="setup-steps" aria-label={t("Application setup")}>
        {stages.map((label, index) => (
          <li key={label} aria-current={step === index ? "step" : undefined}>
            <span>{index + 1}</span>
            {t(label)}
          </li>
        ))}
      </ol>
      <form
        ref={connectForm}
        method="post"
        action="/api/hosting/git/connect"
        hidden
      >
        <input
          name="customerID"
          type="hidden"
          value={options.project.customerID}
        />
        <input name="setupID" type="hidden" value={saved?.id ?? ""} />
      </form>
      <HostingSaveContext.Provider value={environmentResult}>
        <HostingPendingContext.Provider value={pending}>
          <HostingDirtyContext.Provider value={() => setDirty(true)}>
            <form
              action={action}
              className="panel setup-form"
              aria-busy={pending || connecting}
            >
              <input
                type="hidden"
                name="projectID"
                value={options.project.projectID}
              />
              <input type="hidden" name="setupID" value={saved?.id ?? ""} />
              <input
                type="hidden"
                name="revision"
                value={saved?.revision ?? 0}
              />
              <input
                type="hidden"
                name="configuration"
                value={JSON.stringify(configuration)}
              />
              <input type="hidden" name="idempotencyKey" value={key} />
              <input
                type="hidden"
                name="previewID"
                value={saved?.preview?.id ?? ""}
              />
              <button
                type="submit"
                name="intent"
                value={step === 3 ? "deploy" : step === 2 ? "review" : "next"}
                hidden
                disabled={step === 3 && !ready}
                tabIndex={-1}
                aria-hidden="true"
              >
                {t("Continue")}
              </button>
              <fieldset
                disabled={pending || connecting}
                className="setup-fields-root"
              >
                {step === 0 && (
                  <SetupSourceFields
                    configuration={configuration}
                    options={options}
                    onChange={update}
                  />
                )}
                {step === 1 && (
                  <>
                    <h2>{t("Configure your application")}</h2>
                    <div className="setup-fields">
                      <label className="field">
                        {t("Application name")}
                        <input
                          required
                          value={configuration.name}
                          maxLength={160}
                          onChange={(e) =>
                            update({ ...configuration, name: e.target.value })
                          }
                        />
                      </label>
                      <label className="field">
                        {t("HTTP port")}
                        <input
                          type="number"
                          required
                          min={1024}
                          max={65535}
                          value={configuration.spec.port}
                          readOnly={configuration.source.kind === "template"}
                          onChange={(e) =>
                            spec({ port: Number(e.target.value) })
                          }
                        />
                      </label>
                      <label className="field">
                        {t("CPU cores")}
                        <input
                          type="number"
                          required
                          min={0.01}
                          max={64}
                          step={0.01}
                          value={configuration.spec.cpuMillicores / 1000}
                          onChange={(e) =>
                            spec({
                              cpuMillicores: Math.round(
                                Number(e.target.value) * 1000,
                              ),
                            })
                          }
                        />
                      </label>
                      <label className="field">
                        {t("Memory (MiB)")}
                        <input
                          type="number"
                          required
                          min={16}
                          step={16}
                          value={configuration.spec.memoryBytes / 1048576}
                          onChange={(e) =>
                            spec({
                              memoryBytes: Math.round(
                                Number(e.target.value) * 1048576,
                              ),
                            })
                          }
                        />
                      </label>
                    </div>
                    <details>
                      <summary>{t("Advanced settings")}</summary>
                      <div className="setup-fields">
                        <label className="field">
                          {t("Healthcheck path")}
                          <input
                            required
                            value={configuration.spec.healthPath}
                            readOnly={configuration.source.kind === "template"}
                            onChange={(e) =>
                              spec({ healthPath: e.target.value })
                            }
                          />
                        </label>
                        <label className="field">
                          {t("Instances")}
                          <input
                            type="number"
                            required
                            min={1}
                            max={20}
                            value={configuration.spec.replicas}
                            onChange={(e) =>
                              spec({ replicas: Number(e.target.value) })
                            }
                          />
                        </label>
                        <label className="field">
                          {t("Temporary storage (MiB)")}
                          <input
                            type="number"
                            required
                            min={1}
                            value={configuration.spec.ephemeralBytes / 1048576}
                            onChange={(e) =>
                              spec({
                                ephemeralBytes: Math.round(
                                  Number(e.target.value) * 1048576,
                                ),
                              })
                            }
                          />
                        </label>
                        <label className="field">
                          {t("Persistent storage (MiB)")}
                          <input
                            type="number"
                            min={0}
                            value={
                              (configuration.spec.volumeBytes ?? 0) / 1048576
                            }
                            onChange={(e) =>
                              spec({
                                volumeBytes: Math.round(
                                  Number(e.target.value) * 1048576,
                                ),
                              })
                            }
                          />
                          <small>
                            {t(
                              "Use 0 for no volume, or at least 64 MiB at /data. Persistent data is retained when the application stops.",
                            )}
                          </small>
                        </label>
                        {configuration.source.kind !== "template" && (
                          <label className="field">
                            {t("Container arguments")}
                            <textarea
                              value={configuration.spec.args.join("\n")}
                              onChange={(e) =>
                                spec({
                                  args: e.target.value
                                    ? e.target.value.split("\n")
                                    : [],
                                })
                              }
                            />
                            <small>{t("One argument per line.")}</small>
                          </label>
                        )}
                      </div>
                    </details>
                    <ApplicationEnvironment names={saved?.environmentNames} />
                  </>
                )}
                {step === 2 && (
                  <SetupServiceFields
                    configuration={configuration}
                    options={options}
                    onChange={update}
                  />
                )}
                {step === 3 && saved && (
                  <SetupReview
                    view={saved}
                    options={options}
                    accepted={accepted}
                    onAccept={setAccepted}
                  />
                )}
                {state.inspection && step === 0 && (
                  <div className="notice">
                    <strong>{t("Repository suggestions")}</strong>
                    <p>
                      <code>{state.inspection.sourceSHA.slice(0, 12)}</code>
                      {state.inspection.port !== null && (
                        <>
                          {" "}
                          · {t("HTTP port")}: {state.inspection.port}
                        </>
                      )}
                    </p>
                    {state.inspection.suggestedServices.length > 0 && (
                      <p>
                        {t("Suggested services")}:{" "}
                        {state.inspection.suggestedServices
                          .map((service) =>
                            service === "postgres"
                              ? "PostgreSQL"
                              : t("Mail sending"),
                          )
                          .join(", ")}
                      </p>
                    )}
                    {state.inspection.warnings.map((w) => (
                      <p key={w}>{error(w)}</p>
                    ))}
                    {state.inspection.port !== null && (
                      <button
                        type="button"
                        className="button secondary"
                        onClick={() => spec({ port: state.inspection!.port! })}
                      >
                        {t("Use detected port")}
                      </button>
                    )}
                  </div>
                )}
                {state.error && (
                  <p className="notice error" role="alert">
                    {error(state.error)}
                  </p>
                )}
                <div className="setup-footer">
                  {step > 0 && !Object.keys(saved?.resources ?? {}).length && (
                    <button
                      className="button secondary"
                      name="intent"
                      value="back"
                    >
                      {t("Back")}
                    </button>
                  )}
                  {step === 3 && (
                    <button
                      className="button secondary"
                      name="intent"
                      value="refresh"
                    >
                      {t("Refresh")}
                    </button>
                  )}
                  {step < 3 && (
                    <button
                      className="button secondary"
                      name="intent"
                      value="save"
                    >
                      {t("Save draft")}
                    </button>
                  )}
                  <button
                    className="button"
                    name="intent"
                    value={
                      step === 3 ? "deploy" : step === 2 ? "review" : "next"
                    }
                    disabled={step === 3 && !ready}
                  >
                    {pending
                      ? t("Saving…")
                      : connecting
                        ? t("Connecting…")
                        : step === 3
                          ? saved?.preview?.offers.length
                            ? t("Accept order and deploy")
                            : t("Deploy application")
                          : step === 2
                            ? t("Review setup")
                            : t("Continue")}
                  </button>
                </div>
              </fieldset>
            </form>
          </HostingDirtyContext.Provider>
        </HostingPendingContext.Provider>
      </HostingSaveContext.Provider>
      {step === 2 && (
        <details className="workspace-disclosure">
          <summary>{t("Set up a Mail connection or sender domain")}</summary>
          {mailSetup}
          <button
            type="button"
            className="button secondary"
            onClick={() => router.refresh()}
          >
            {t("Refresh available services")}
          </button>
        </details>
      )}
      {saved && (
        <p className="help">
          {t("Your draft is saved in this project. You can continue it later.")}
        </p>
      )}
    </div>
  );
}

export function SetupWizard(props: ComponentProps<typeof SetupWizardState>) {
  return <SetupWizardState key={hostingUIScope(props)} {...props} />;
}
