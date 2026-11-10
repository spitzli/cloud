import type { HostingPorts } from "./ports";
import { msgid } from "@spitzli/i18n";
import {
  HostingError,
  environmentPatchSchema,
  type EnvironmentPatch,
} from "@spitzli/hosting-contracts";
export type Dependencies = {
  encryptMailSecret: HostingPorts["encryptMailSecret"];
  decryptMailSecret: HostingPorts["decryptMailSecret"];
};

export function createHostingEnvironment(dependencies: Dependencies) {
  function patchEnvironment(
    current: Record<string, string>,
    patch: EnvironmentPatch,
  ): Record<string, string> {
    const next = { ...current };
    for (const item of environmentPatchSchema.parse(patch)) {
      if (item.value === null) delete next[item.name];
      else next[item.name] = item.value;
    }
    environmentPatchSchema.parse(
      Object.entries(next).map(([name, value]) => ({ name, value })),
    );
    return next;
  }
  function openEnvironment(
    appID: string,
    sealed?: string | null,
  ): Record<string, string> {
    if (!sealed) return {};
    try {
      const value = dependencies.decryptMailSecret<Record<string, string>>(
        sealed,
        "hosting-env:" + appID,
      );
      return patchEnvironment(
        {},
        Object.entries(value).map(([name, value]) => ({ name, value })),
      );
    } catch {
      throw new HostingError(
        503,
        msgid("Stored application environment could not be opened."),
      );
    }
  }
  function sealEnvironment(
    appID: string,
    values: Record<string, string>,
  ): string | null {
    const checked = patchEnvironment(
      {},
      Object.entries(values).map(([name, value]) => ({ name, value })),
    );
    return Object.keys(checked).length
      ? dependencies.encryptMailSecret(checked, "hosting-env:" + appID)
      : null;
  }
  function redactValues(
    logs: string,
    values: string[],
    truncated = false,
  ): string {
    const candidates = [
      ...new Set(values.flatMap((value) => [value, ...value.split(/\r?\n/)])),
    ]
      .filter(Boolean)
      .sort((a, b) => b.length - a.length);
    // Byte-bounded logs can begin/end inside a configured value. Remove those
    // edge fragments before returning even a partial token to the client.
    for (const value of truncated ? candidates : []) {
      for (let n = Math.min(value.length - 1, logs.length); n > 0; n--) {
        if (logs.endsWith(value.slice(0, n))) {
          logs = logs.slice(0, -n) + "[redacted]";
          break;
        }
      }
      for (let n = Math.min(value.length - 1, logs.length); n > 0; n--) {
        if (logs.startsWith(value.slice(-n))) {
          logs = "[redacted]" + logs.slice(n);
          break;
        }
      }
    }
    for (const value of candidates) logs = logs.split(value).join("[redacted]");
    return logs;
  }
  return { patchEnvironment, openEnvironment, sealEnvironment, redactValues };
}
