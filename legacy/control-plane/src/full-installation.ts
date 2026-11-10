import { hostingManifest } from "@spitzli/module-hosting/manifest";
import { databasesManifest } from "@spitzli/module-databases/manifest";
import { storageManifest } from "@spitzli/module-storage/manifest";
import { mailManifest, isMailProvisioning } from "@spitzli/module-mail/manifest";
import { configureInstallation } from "./installation";
export const fullManifests = [mailManifest, hostingManifest, databasesManifest, storageManifest];
export function configureFullInstallation() { configureInstallation(fullManifests, isMailProvisioning); }
