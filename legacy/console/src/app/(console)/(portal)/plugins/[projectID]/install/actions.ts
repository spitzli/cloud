"use server";

import type {
  PluginImport,
  PluginSourceCommand,
  PluginSourceView,
} from "@webdock/api-client/plugin-sources";
import { requireRegistryOperator } from "@/lib/registry-page";
import { studioCall } from "@/lib/studio-client";
import { PluginSourceError, StudioError } from "@/lib/studio-errors";

export async function pluginSourceAction(
  command: PluginSourceCommand,
): Promise<
  | { ok: true; data: PluginImport | PluginSourceView }
  | { ok: false; error: string; status?: number }
> {
  await requireRegistryOperator();
  try {
    return {
      ok: true,
      data: await studioCall<PluginImport | PluginSourceView>(
        "plugins.sources",
        [command],
      ),
    };
  } catch (error) {
    // Never reflect Git credentials, archive contents, or upstream errors into the browser.
    return {
      ok: false,
      error:
        error instanceof PluginSourceError
          ? error.message
          : "The installer request could not be completed. Refresh the import status before retrying.",
      ...(error instanceof StudioError ? { status: error.status } : {}),
    };
  }
}
