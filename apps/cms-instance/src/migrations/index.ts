import * as migration_20261018_190151_initial from "./20261018_190151_initial";
import * as migration_20261018_201453_managed_media_trash from "./20261018_201453_managed_media_trash";
import * as mediaStorage from "./20261025_120000_media_storage_binding";

export const migrations = [
  {
    up: migration_20261018_190151_initial.up,
    down: migration_20261018_190151_initial.down,
    name: "20261018_190151_initial",
  },
  {
    up: migration_20261018_201453_managed_media_trash.up,
    down: migration_20261018_201453_managed_media_trash.down,
    name: "20261018_201453_managed_media_trash",
  },
  {
    up: mediaStorage.up,
    down: mediaStorage.down,
    name: "20261025_120000_media_storage_binding",
  },
];
