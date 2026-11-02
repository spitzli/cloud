import { useEffect, useRef } from "react";
import {
  parseLaunch,
  parsePanel,
  validateActionMessage,
  type Launch,
} from "./contracts";
import type { InvokeAction } from "./Panel";

/** Invoke must reauthorize installation, project grant and action on the server. */
export function CustomPluginUI({
  launch: raw,
  coreOrigins,
  invoke,
  active,
}: {
  launch: unknown;
  coreOrigins: string[];
  invoke: InvokeAction;
  active: boolean;
}) {
  let launch: Launch;
  try {
    launch = parseLaunch(raw, [
      ...coreOrigins,
      ...(typeof window === "undefined" ? [] : [window.location.origin]),
    ]);
  } catch {
    return (
      <p role="alert" className="text-danger">
        This extension cannot open a secure view.
      </p>
    );
  }
  if (!active) return <p role="status">Extension access is unavailable.</p>;
  return (
    <IsolatedFrame
      key={`${launch.installationId}:${launch.url}`}
      launch={launch}
      invoke={invoke}
    />
  );
}
function IsolatedFrame({
  launch,
  invoke,
}: {
  launch: Launch;
  invoke: InvokeAction;
}) {
  const ref = useRef<HTMLIFrameElement>(null);
  const actionsKey = launch.actions.join("\n");
  useEffect(() => {
    const controller = new AbortController();
    const seen = new Set<string>();
    let pending = 0;
    const origin = new URL(launch.url).origin;
    const listener = async (event: MessageEvent) => {
      const frame = ref.current?.contentWindow;
      const message = validateActionMessage(event, {
        source: frame,
        origin,
        installationId: launch.installationId,
        actions: actionsKey.split("\n"),
        active: !controller.signal.aborted,
      });
      if (
        !message ||
        !frame ||
        seen.has(message.requestId) ||
        seen.size >= 256 ||
        pending >= 4
      )
        return;
      seen.add(message.requestId);
      pending++;
      const reply = {
        version: 1,
        type: "webdock.result",
        requestId: message.requestId,
        installationId: launch.installationId,
      };
      try {
        // Only the display contract crosses this boundary, never a raw API/session response.
        const panel = parsePanel(
          await invoke(message.action, message.input, controller.signal),
        );
        if (!controller.signal.aborted && frame === ref.current?.contentWindow)
          frame.postMessage({ ...reply, panel }, origin);
      } catch {
        if (!controller.signal.aborted && frame === ref.current?.contentWindow)
          frame.postMessage({ ...reply, error: "Action unavailable" }, origin);
      } finally {
        pending--;
      }
    };
    const handleMessage = (event: MessageEvent) => {
      void listener(event);
    };
    window.addEventListener("message", handleMessage);
    return () => {
      controller.abort();
      window.removeEventListener("message", handleMessage);
    };
  }, [launch.installationId, launch.url, actionsKey, invoke]);
  return (
    <iframe
      ref={ref}
      title="Extension view"
      className="min-h-[480px] w-full rounded-xl border border-outline bg-surface"
      src={launch.url}
      sandbox="allow-scripts allow-same-origin"
      referrerPolicy="no-referrer"
      allow="camera 'none'; microphone 'none'; geolocation 'none'; clipboard-read 'none'; clipboard-write 'none'"
    />
  );
}
