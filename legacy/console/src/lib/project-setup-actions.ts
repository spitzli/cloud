"use server";
import type { SetupActionState } from "@webdock/module-hosting/ui-types";
export type { SetupActionState } from "@webdock/module-hosting/ui-types";
import { revalidatePath } from "next/cache";
import {
  hostingError,
  setupCommandSchema,
  type SetupView,
  type SetupInspection,
} from "@webdock/hosting-contracts";
import { hostingCall } from "./hosting-client";

export async function saveSetup(
  _previous: SetupActionState,
  form: FormData,
): Promise<SetupActionState> {
  let view: SetupView | undefined;
  try {
    if (form.get("intent") === "refresh") {
      view = await hostingCall<SetupView>(
        setupCommandSchema.parse({
          action: "setup.preview",
          projectID: String(form.get("projectID")),
          setupID: String(form.get("setupID")),
          revision: Number(form.get("revision")),
        }),
      );
      revalidatePath(`/hosting/projects/${view.projectID}`, "layout");
      return { view };
    }
    const configuration = String(form.get("configuration") ?? "");
    const environment = String(form.get("environment") ?? "[]");
    if (configuration.length > 16000 || environment.length > 32000)
      throw Error("Input limit");
    view = await hostingCall<SetupView>(
      setupCommandSchema.parse({
        action: "setup.save",
        projectID: String(form.get("projectID")),
        setupID: form.get("setupID") ? String(form.get("setupID")) : undefined,
        revision: Number(form.get("revision")),
        configuration: JSON.parse(configuration),
        environment: JSON.parse(environment),
        idempotencyKey: String(form.get("idempotencyKey")),
      }),
    );
    let inspection: SetupInspection | undefined;
    if (form.get("intent") === "inspect" && form.get("setupID"))
      inspection = await hostingCall<SetupInspection>({
        action: "setup.inspectSource",
        projectID: view.projectID,
        setupID: view.id,
        revision: view.revision,
      });
    if (form.get("intent") === "review")
      view = await hostingCall<SetupView>({
        action: "setup.preview",
        projectID: view.projectID,
        setupID: view.id,
        revision: view.revision,
      });
    revalidatePath(`/hosting/projects/${view.projectID}`, "layout");
    return { view, inspection };
  } catch (error) {
    return { view, error: hostingError(error).message };
  }
}
export async function confirmSetup(
  _previous: SetupActionState,
  form: FormData,
): Promise<SetupActionState> {
  try {
    const view = await hostingCall<SetupView>(
      setupCommandSchema.parse({
        action: "setup.confirm",
        projectID: String(form.get("projectID")),
        setupID: String(form.get("setupID")),
        revision: Number(form.get("revision")),
        previewID: String(form.get("previewID")),
        acceptedOfferIDs: form.getAll("acceptedOfferIDs").map(String),
        idempotencyKey: String(form.get("idempotencyKey")),
      }),
    );
    revalidatePath(`/hosting/projects/${view.projectID}`, "layout");
    return { view };
  } catch (error) {
    return { error: hostingError(error).message };
  }
}
