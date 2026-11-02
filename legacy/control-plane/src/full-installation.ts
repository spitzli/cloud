import { hostingManifest } from "@webdock/module-hosting/manifest";
import { databasesManifest } from "@webdock/module-databases/manifest";
import { storageManifest } from "@webdock/module-storage/manifest";
import { mailManifest, isMailProvisioning } from "@webdock/module-mail/manifest";
import { configureInstallation } from "./installation";
export const fullManifests = [mailManifest, hostingManifest, databasesManifest, storageManifest];
export function configureFullInstallation() { configureInstallation(fullManifests, isMailProvisioning); }
