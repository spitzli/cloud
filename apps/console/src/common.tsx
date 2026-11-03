import { ArrowClockwiseIcon } from "@phosphor-icons/react/dist/csr/ArrowClockwise";
import { WarningCircleIcon } from "@phosphor-icons/react/dist/csr/WarningCircle";
import { useEffect, useRef, type ReactNode } from "react";
import { useOptionalI18n } from "@webdock/i18n/react";
import * as ui from "./ui";
import { describeFailure } from "./AuthFrame";
const state = "flex items-start gap-3 rounded-lg border border-outline bg-surface px-5 py-6 max-sm:px-4 max-sm:py-5";
export function Loading({ children = "Loading…" }: { children?: ReactNode }) {
  return (
    <div className={`${state} text-muted`} role="status">
      <span className="mt-[7px] size-2 shrink-0 animate-pulse rounded-full bg-muted" />
      {children}
    </div>
  );
}
export function Failure({
  error,
  retry,
  title = "Unable to load",
}: {
  error: Error;
  retry?: () => void;
  title?: string;
}) {
  const i18n = useOptionalI18n();
  const message = describeFailure(error);
  return (
    <div className={`${state} text-danger`} role="alert">
      <WarningCircleIcon size={22} className="shrink-0" />
      <div>
        <strong className="font-semibold">{i18n?.t(title) ?? title}</strong>
        <p className="mt-1.5">{i18n?.t(message) ?? message}</p>
        {retry ? (
          <button className={ui.cx(`${ui.button()} mt-4`)} onClick={retry}>
            <ArrowClockwiseIcon />
            {i18n?.t("Try again") ?? "Try again"}
          </button>
        ) : null}
      </div>
    </div>
  );
}
export function Empty({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <div className={state}>
      <div>
        <h2 className={ui.cx(ui.sectionTitle, "min-w-0 [overflow-wrap:anywhere]")}>{title}</h2>
        <p className="mt-1.5 text-muted">{children}</p>
      </div>
    </div>
  );
}
export function Modal({
  title,
  close,
  children,
  dismissible = true,
}: {
  title: string;
  close: () => void;
  children: ReactNode;
  /** False for steps that must be completed explicitly, such as saving one-time codes. */
  dismissible?: boolean;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current;
    const previous = document.activeElement as HTMLElement | null;
    dialog?.showModal();
    return () => {
      dialog?.close();
      previous?.focus();
    };
  }, []);
  return (
    <dialog
      ref={ref}
      aria-label={title}
      className="m-auto max-h-[calc(100dvh-32px)] w-[min(570px,calc(100vw-32px))] overflow-y-auto rounded-xl border border-outline bg-surface p-5 text-foreground shadow-2xl backdrop:bg-black/45"
      onCancel={(event) => {
        event.preventDefault();
        if (dismissible) close();
      }}
      onClick={(event) => {
        if (!dismissible || event.target !== event.currentTarget) return;
        const bounds = event.currentTarget.getBoundingClientRect();
        if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) close();
      }}
    >
      <div className="mb-4 flex items-center justify-between gap-3">
        <h2 className={ui.cx(ui.sectionTitle, "min-w-0 [overflow-wrap:anywhere]")}>{title}</h2>
        {dismissible ? (
          <button
            className={ui.button("quiet")}
            onClick={close}
            aria-label={`Close ${title}`}
          >
            Close
          </button>
        ) : null}
      </div>
      {children}
    </dialog>
  );
}
export function Status({ status }: { status?: string | undefined }) {
  return (
    <span className={ui.badge(ui.statusTone(status))}>
      {status ?? "Unknown"}
    </span>
  );
}
