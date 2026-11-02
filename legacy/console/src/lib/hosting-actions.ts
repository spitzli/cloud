"use server";
import type { HostingFormState } from "@webdock/module-hosting/ui-types";
export type { HostingFormState } from "@webdock/module-hosting/ui-types";
import { msgid } from "@webdock/i18n";
import { revalidatePath } from "next/cache";
import {
  commandSchema,
  parseResourceInput,
  hostingDimensions,
  hostingError,
  HostingError,
  parseHostingFields,
} from "@webdock/hosting-contracts";
import { hostingCall } from "./hosting-client";

export async function saveHosting(
  _previous: HostingFormState,
  form: FormData,
): Promise<HostingFormState> {
  let mailMutation = false;
  try {
    const raw = String(form.get("command") ?? "");
    if (raw.length > 16000) throw Error("Oversized command");
    const base = JSON.parse(raw);
    mailMutation =
      typeof base?.action === "string" && base.action.startsWith("byok.mail.");
    const cmd: Record<string, unknown> = { ...base };
    for (const field of [
      "name",
      "provider",
      "country",
      "region",
      "locationEvidence",
      "dedicatedCustomerID",
      "projectID",
      "clusterID",
      "mode",
    ])
      if (form.has(field)) {
        const value = String(form.get(field));
        cmd[field] = value || (field === "country" ? null : undefined);
      }
    if (form.has("ownImagesPresent"))
      cmd.ownImages = form.get("ownImages") === "yes";
    if (base.action === "git.source.configure") {
      for (const field of [
        "connectionID",
        "repositoryID",
        "branch",
        "rootDirectory",
        "healthPath",
        "recipe",
        "targetID",
        "buildProvider",
        "workflowPath",
        "artifactPrefix",
      ]) {
        if (form.has(field)) cmd[field] = String(form.get(field));
      }
      cmd.enabled = form.get("enabled") === "yes";
      cmd.autoPublish = form.get("autoPublish") === "yes";
      if (form.has("buildEnvironment")) {
        const value = String(form.get("buildEnvironment"));
        if (value.length > 131072)
          throw new HostingError(400, "Build environment is too large.");
        cmd.buildEnvironment = JSON.parse(value);
      }
    }
    if (base.action === "git.targets.bind") {
      cmd.targetID = String(form.get("targetID") ?? "");
      cmd.mode = String(form.get("mode") ?? base.mode);
    }
    if (base.action === "projects.create" || base.action === "projects.update")
      cmd.confirmSharedImages = form.get("confirmSharedImages") === "yes";
    if (base.action === "byok.policy.set") {
      for (const key of ["kubernetes", "vercel", "turbosmtp", "manageExisting"])
        cmd[key] = form.get(key) === "yes";
      cmd.maxClusters = Number(form.get("maxClusters"));
      if (form.has("maxMailDomains"))
        cmd.maxMailDomains = Number(form.get("maxMailDomains"));
      cmd.namespaces = String(form.get("namespaces") ?? "")
        .split(/[\s,]+/)
        .filter(Boolean);
    }
    if (base.action === "byok.vercel.select")
      cmd.projects = form.getAll("projects").map(String);
    if (base.action === "byok.workload" && form.has("replicas"))
      cmd.replicas = Number(form.get("replicas"));
    if (base.action === "byok.mail.connect") {
      cmd.label = String(form.get("label") ?? "");
      cmd.consumerKey = String(form.get("consumerKey") ?? "").trim();
      cmd.consumerSecret = String(form.get("consumerSecret") ?? "").trim();
      cmd.confirmAccountAccess = form.get("confirmAccountAccess") === "yes";
    }
    if (base.action === "byok.mail.domain")
      cmd.domain = String(form.get("domain") ?? "");
    if (base.action === "byok.mail.review")
      cmd.confirmDomain = String(form.get("confirmDomain") ?? "");
    if (base.action === "limits.set") {
      const input: Record<string, string> = {};
      for (const k of hostingDimensions)
        if (form.has(`hosting.${k}`))
          input[`hosting.${k}`] = String(form.get(`hosting.${k}`));
      try {
        cmd.values = parseHostingFields(input);
      } catch {
        throw new HostingError(
          400,
          "Enter valid resource amounts in cores, MB or GB.",
        );
      }
      const scope = String(form.get("scope") ?? "customer");
      delete cmd.provider;
      delete cmd.projectID;
      if (scope === "provider:k3s" || scope === "provider:vercel")
        cmd.provider = scope.split(":")[1];
      else if (scope.startsWith("project:")) cmd.projectID = scope.slice(8);
      else if (scope !== "customer") throw Error("Invalid scope");
    }
    if (base.action === "clusters.activate") {
      const input: Record<string, string> = {};
      for (const k of hostingDimensions)
        input[`hosting.${k}`] = String(form.get(`hosting.${k}`) ?? "0");
      try {
        cmd.capacity = parseHostingFields(input);
      } catch {
        throw new HostingError(
          400,
          "Enter valid resource amounts in cores, MB or GB.",
        );
      }
      cmd.verificationEvidence = String(form.get("verificationEvidence") ?? "");
    }
    if (base.action === "apps.create" || base.action === "apps.update") {
      if (form.has("environment")) {
        const rawEnvironment = String(form.get("environment"));
        if (rawEnvironment.length > 32000)
          throw new HostingError(400, "Environment variables are too large.");
        const environment = JSON.parse(rawEnvironment);
        if (!Array.isArray(environment))
          throw new HostingError(400, "Invalid environment variables.");
        if (environment.length) cmd.environment = environment;
      }
      const spec = { ...base.spec };
      for (const k of ["template", "image", "healthPath"])
        if (form.has(k)) spec[k] = String(form.get(k));
      for (const k of [
        "port",
        "replicas",
        "cpuMillicores",
        "memoryBytes",
        "ephemeralBytes",
        "volumeBytes",
      ])
        if (form.has(k)) {
          try {
            spec[k] = parseResourceInput(k, String(form.get(k)));
          } catch {
            throw new HostingError(
              400,
              "Enter valid resource amounts in cores, MB or GB.",
            );
          }
        }
      if (form.has("args")) spec.args = JSON.parse(String(form.get("args")));
      cmd.spec = spec;
    }
    if (base.action === "apps.scale")
      cmd.replicas = Number(form.get("replicas"));
    if (base.action === "databases.plans.setAvailability")
      cmd.availability = String(form.get("availability"));
    if (
      base.action === "databases.purgeMigrationSource" ||
      base.action === "databases.delete" ||
      base.action === "databases.purge" ||
      base.action === "apps.delete" ||
      base.action === "apps.purgeStorage"
    )
      cmd.confirmName = String(form.get("confirmName") ?? "");
    const result = await hostingCall<{
      setupPath?: string;
      appID?: string;
      id?: string;
      operationID?: string;
      status?: string;
      revision?: number;
    }>(commandSchema.parse(cmd));
    revalidatePath("/infrastructure", "layout");
    revalidatePath("/hosting", "layout");
    revalidatePath("/customers", "layout");
    revalidatePath("/tenants", "layout");
    return {
      environmentReset: base.action === "apps.create",
      revision: result.revision,
      message:
        (
          {
            "byok.mail.connect": msgid(
              "turboSMTP connection verified and saved.",
            ),
            "byok.mail.refresh": msgid("Sender-domain status updated."),
            "byok.mail.domain": msgid(
              "Sender domain registered. Complete DNS setup in turboSMTP.",
            ),
            "byok.mail.disconnect": msgid(
              "turboSMTP disconnected. Your provider resources remain intact.",
            ),
            "byok.mail.review": msgid(
              "Provider state reviewed. You can continue.",
            ),
          } as Record<string, string>
        )[base.action] ??
        (result.status === "queued"
          ? "Hosting operation queued."
          : "Hosting settings saved."),
      operationPath: result.operationID
        ? `/hosting/operations/${result.operationID}`
        : undefined,
      clusterPath:
        base.action === "byok.register" && result.id
          ? `/hosting/clusters/${result.id}`
          : undefined,
      appPath: result.appID ? `/hosting/apps/${result.appID}` : undefined,
      setupPath: result.setupPath,
    };
  } catch (error) {
    if (mailMutation) revalidatePath("/tenants", "layout");
    return { error: hostingError(error).message };
  }
}
export async function displayEnrollment(
  enrollmentID: string,
): Promise<HostingFormState> {
  try {
    const data = await hostingCall<{ token: string; expiresAt: string }>({
      action: "enrollment.display",
      enrollmentID,
    });
    return {
      ...data,
      message: "Save this enrollment token now. It is displayed only once.",
    };
  } catch (error) {
    return { error: hostingError(error).message };
  }
}
