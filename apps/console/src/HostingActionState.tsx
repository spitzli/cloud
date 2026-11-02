import { ApiError } from "./api";
import * as ui from "./ui";
export function ActionFailure({
  error,
  uncertain,
  retry,
  pending,
}: {
  error: Error | null;
  uncertain: boolean;
  retry: () => void;
  pending: boolean;
}) {
  if (!error) return null;
  return (
    <div className="rounded-lg border border-danger/40 bg-danger-soft px-4 py-3 text-[13px] text-danger" role="alert">
      <div className="grid gap-1.5">
        <h2 className="text-sm font-semibold">
          {uncertain
            ? "Request outcome is unknown"
            : "Request was not accepted"}
        </h2>
        <p>{error.message}</p>
        {error instanceof ApiError && error.status === 409 ? (
          <p>
            Refresh and review the current revision and operation before
            requesting another change.
          </p>
        ) : null}
        {uncertain ? (
          <>
            <p>
              Review the current server state. Retrying here sends the same
              request with the same identity; it does not create a new action.
            </p>
            <button
              type="button"
              className={ui.cx(`${ui.button()} mt-1 justify-self-start`)}
              onClick={retry}
              disabled={pending}
            >
              {pending ? "Checking…" : "Retry same request"}
            </button>
          </>
        ) : null}
      </div>
    </div>
  );
}
