"use server";
import { studioCall } from "@/lib/studio-client";
export async function invokePlugin(input: {
  pluginID: string;
  projectID: string;
  action: string;
  input: unknown;
  idempotencyKey: string;
}) {
  try {
    return { result: await studioCall("plugins.invoke", [input]) };
  } catch {
    return {
      error:
        "Plugin action could not be completed. Check access and configuration, then retry.",
    };
  }
}
