import { msgid } from "@webdock/i18n";

// Service errors and stored status codes are translated only at the presentation boundary.
export const gitDeploymentMessages = [
  msgid("An exact commit is required."),
  msgid("Bind Vercel target"),
  msgid("Build environment exceeds the allowed size."),
  msgid("Build environment is too large."),
  msgid("Build lease expired."),
  msgid(
    "Build variables must be explicitly approved and cannot contain provider credentials.",
  ),
  msgid("Build worker endpoint is unavailable."),
  msgid("Complete GitHub authorization first."),
  msgid(
    "Configure Frankfurt Functions and disable cross-region failover before publication.",
  ),
  msgid("Configure Node.js 24 for this Vercel build target."),
  msgid("Configure a Vercel hosting project first."),
  msgid("Configure an explicitly bound platform Vercel deployment credential."),
  msgid("Configure the platform Vercel team first."),
  msgid("Configure verified EU artifact and registry storage."),
  msgid("Connect GitHub and enable a verified source first."),
  msgid("Connect GitHub interactively in Studio."),
  msgid("Customer Vercel connection"),
  msgid(
    "Disconnect native Vercel Git builds before enabling Webdock publication.",
  ),
  msgid("Enter valid resource amounts in cores, MB or GB."),
  msgid("Environment variables are too large."),
  msgid("Git deployment changed. Refresh and try again."),
  msgid(
    "Git deployment changed. Review the source and release before reconciliation.",
  ),
  msgid("Git deployment is unavailable."),
  msgid("Git publication authorization changed. Review the release."),
  msgid(
    "GitHub access could not be verified. Check installation and repository permissions.",
  ),
  msgid("GitHub authorization was already used."),
  msgid("GitHub check reconciliation exceeds the supported limit."),
  msgid("GitHub connection expired. Start again."),
  msgid("GitHub connection setup is incomplete."),
  msgid(
    "GitHub could not complete this request. Check the current state before retrying.",
  ),
  msgid("GitHub deployments are not configured yet."),
  msgid("GitHub event is too large."),
  msgid("GitHub installation is unavailable."),
  msgid("GitHub repository selection exceeds the supported limit."),
  msgid("GitHub webhooks are not configured yet."),
  msgid("Install verified registry pull access on the target cluster."),
  msgid("Invalid build lease."),
  msgid("Invalid build result."),
  msgid("Invalid build worker."),
  msgid("Invalid delivery."),
  msgid("Invalid environment variables."),
  msgid("Invalid immutable artifact."),
  msgid("Invalid immutable registry image."),
  msgid("Invalid publication result."),
  msgid("Multiple GitHub checks match this build. Reconciliation is required."),
  msgid("Platform Vercel connection"),
  msgid("Publication identity cannot be replaced."),
  msgid("Publication requires observed health and verified target location."),
  msgid("Reconnect Vercel with deployment write and project read permissions."),
  msgid("Release health has not been verified."),
  msgid("Renew approval"),
  msgid(
    "Rollback requires a retained healthy release with compatible configuration.",
  ),
  msgid("Runtime configuration changed. Build again before publication."),
  msgid(
    "Select a Vercel connection and project before enabling Git deployments.",
  ),
  msgid("Select a Vercel deployment target before enabling Git deployments."),
  msgid(
    "Select an existing authorized Vercel project. Disable native Git builds and configure Frankfurt Functions before binding.",
  ),
  msgid("Select this project in the customer Vercel connection first."),
  msgid("Ten builds are already queued or running for this project."),
  msgid("This GitHub installation belongs to another customer."),
  msgid(
    "This Vercel target is already bound to another hosting project or connection.",
  ),
  msgid("Vercel deployment identity could not be verified."),
  msgid("Vercel deployment target could not be verified."),
  msgid("Vercel project ID"),
  msgid("Verified EU microVM capacity is required."),
  msgid("Verify and bind target"),
  msgid("Wait before connecting GitHub again."),
  msgid("active"),
  msgid("awaiting-approval"),
  msgid("cancelled"),
  msgid("completed"),
  msgid("deploying"),
  msgid("disconnected"),
  msgid("failed"),
  msgid("in_progress"),
  msgid("needs-reconciliation"),
  msgid("queued"),
  msgid("ready"),
  msgid("revalidation-required"),
  msgid("running"),
  msgid("succeeded"),
  msgid("superseded"),
];

msgid("Configure the GitHub App ID and private key in the private Auth environment.");

msgid("Configure the GitHub webhook secret before enabling push builds.");

msgid("Enroll verified EU isolated build capacity before requesting a build.");

msgid("Verified isolated build capacity is required.");

msgid("Enroll verified isolated build capacity before requesting a build.");

msgid("Configure verified artifact and registry storage.");

msgid("The original Vercel target is no longer accessible.");

msgid("The original publication target could not be verified.");

msgid("Enroll a trusted artifact publisher before importing Actions builds.");

msgid("Configure build variables in GitHub Actions before using external builds.");

msgid("Accept the GitHub App Actions read permission first.");

msgid("Run the configured workflow in GitHub Actions. Webdock imports successful builds automatically.");

msgid("Verified build or artifact publisher capacity is required.");

msgid("Actions imports cannot request source archives.");

msgid("External build variables are unsupported.");
