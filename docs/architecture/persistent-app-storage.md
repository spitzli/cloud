# Fixed-size persistent app storage

Status, 2026-10-09: deployed and activated on Contabo after explicit user approval. Auth `dpl_G4z5ehmcbAtqYTi3d98DEhooA5Y1` and Studio `dpl_3SuxCpksRT9pmhjYvN952r12qMfs` are READY with fra1 Functions after local prebuilt builds. The private helper runs in the real host mount namespace and the agent retains its sandbox. A 20 GB pool with 20 GB host reserve is active. Lunares uses a 1 GB volume and the user's uploaded SQLite snapshot; original contents survived a real app restart. See `docs/verification/2026-10-09-lunares-production.json` for evidence. Discord token/login remain user-supplied and pending.

## User flow

An app can request persistent capacity at creation. Zero disables it; positive values must be at least 64 MiB (the decimal UI recommends 68 MB). The fixed filesystem is mounted at `/data`, supports at most one replica, and remains charged when the app stops or is deleted. Its usable capacity is smaller than the reserved image because ext4 needs metadata/journal space. App updates and rollbacks change the container configuration, never restore an older version of its data.

Deleted apps with retained data remain in the app list. A platform administrator opens the separate retained-data deletion page, reviews the current plan and confirms the exact app name. The existing operation journal removes owned PVC/PV objects, verifies that no pod uses the claim, unmounts the filesystem and deletes the image. Only a matching completion proof releases bytes. Customer preview, ordinary customer identities and stale deletion plans cannot purge data.

REST adds `GET /api/hosting/apps/{id}/storage-deletion` and `DELETE /api/hosting/apps/{id}/storage`. MCP adds `preview_hosting_storage_deletion` and `delete_hosting_storage`. App creation/update schemas accept optional `volumeBytes`; changing it after creation is rejected. Existing apps without the field are compatible. No database migration is needed: app specs and journal payloads already use JSON.

## Node implementation

The normal hosting agent retains its existing capability and filesystem sandbox. `storage_client.py` calls a root-owned mode-0600 Unix socket. `storage_service.py` checks peer UID, input bounds, IDs, size and lease, then serializes storage operations. The helper's systemd unit runs in the host mount namespace, so kubelet sees the mounted filesystems; it has CAP_SYS_ADMIN, CAP_CHOWN, CAP_DAC_OVERRIDE and CAP_FOWNER. This is a privileged trusted platform component, not a customer shell or API. It has no IP listener and uses no external storage service.

Each image is fully preallocated. Creation rejects insufficient configured capacity or insufficient host free space after an explicit OS reserve. mkfs uses `nodiscard` to preserve reservation and completes initialization. Static Local PVs use node affinity, an explicit claim binding and `Retain`. PVCs are allowed only within the reserved namespace storage quota. The PV path ends in `mount/data`: that directory exists inside the image only, so an absent mount cannot become unbounded writes to the host.

The root-owned pool journal is separate from individual app directories and is bound to an explicit pool UUID. Allocation is recorded before filesystem creation. Missing directories/images, wrong owners, unexpected files and UUID changes fail closed. A revision-one retry cannot silently recreate missing data. A `purging` journal state distinguishes interrupted authorized deletion from missing storage. Filesystem/directory synchronization precedes publishing `ready` or `purged`; allocation tombstones are retained.

The helper holds a process lock and restores known mounts before systemd readiness notification. Failed mounts suppress advertised storage readiness; the helper remains reachable for explicit recovery. Delete/purge can still operate with a storage-version-capable agent when normal storage readiness is false. Loss of the pool journal requires restoration and operator investigation, not automatic initialization.

## Production installation recipe

The following installation was executed on Contabo on 2026-10-09. The backup is `/root/setup-backup/webdock-storage-20261009/`. This is a reference for future installations, not permission to rerun initialization on the existing pool. Verify live state and preserve existing files before any subsequent rollout.

1. Install `storage.py`, `storage_service.py`, `storage_client.py` and the updated executor/observer/connect modules under `/opt/webdock-hosting-agent`, root-owned. Install `webdock-storage.service`. Existing Python, e2fsprogs, util-linux and coreutils supply mkfs, blkid, mount, losetup, findmnt and sync; verify their availability locally without unverified remote downloads.
2. Create `/var/lib/webdock-storage` as root:root mode 0700. Write `/etc/webdock-storage.json` mode 0600 with the real node/cluster, a new UUID and finite capacities. A proposed initial pool is 20 GB with a 20 GB host free-space reserve; this is a proposed configuration, not an existing entitlement:

   ```json
   {"root":"/var/lib/webdock-storage","node":"vmd208517","clusterID":"101775802667896832","poolID":"<new UUID>","capacityBytes":20000000000,"reserveBytes":20000000000}
   ```

3. Run `python3 /opt/webdock-hosting-agent/storage.py initialize` exactly once for the new empty pool. The command refuses an existing/nonempty pool. Preserve and back up `_pool/state.json` with its images; never initialize over missing production metadata.
4. Add a hosting-agent unit drop-in with `Requires=webdock-storage.service` and `After=webdock-storage.service`. Enable the helper and reload/restart the normal agent. Verify helper readiness, host/helper mount namespace identity, root-only socket access and fresh heartbeat storageVersion=1. No reboot is needed to install; cold-boot behavior still needs a scheduled real-node check before claiming it verified there.
5. Build Studio/Auth locally with Bun, publish only through the already-authorized Frankfurt prebuilt workflow when deployment is requested, and verify rendered pages. Then a platform administrator can activate persistent cluster capacity within the fresh advertised bound. Customer allowances require separate explicit changes; Lunares remains at zero until source data inventory determines its requirement.

Rollback of UI/control-plane changes must keep the helper, images and pool journal intact once any app uses storage. Do not stop a required mount service or remove its files as a deployment rollback.

## Verification and limits

The disposable local minikube node verified a real systemd helper in PID 1's mount namespace and a capability-less executor with strict filesystem protection. UI creation, stop/start, cold remount via helper restart, app deletion/retention and separate purge completed against the local preview database. A filesystem probe filled 128 MiB until ENOSPC after 116,391,936 written bytes, checked SQLite backup/restore, rejected changed ownership and lost images/directories, and retried an injected crash after image unlink. No physical power-loss test is claimed.

The local cached nginx fixture used its cache tag in the local adapter after verifying identical image layers, architecture, OS and configuration against the known digest image. Production's immutable-image validation and `imagePullPolicy: Never` remain unchanged. No image was pulled or built remotely.

This is local single-node storage, not replicated storage or high availability. There is no scheduled backup service, cross-node migration, volume resizing, multiwriter support or data-restore UI in this increment. A tested SQLite backup/restore probe is not a deployed backup service. The full host reboot and storage activation on Contabo remain unverified. The existing EU restrictions and Vercel global-service caveats remain unchanged.

Implementation references: [Local PV ownership and Retain](https://kubernetes.io/docs/concepts/storage/persistent-volumes/), [mkfs.ext4 allocation options](https://man7.org/linux/man-pages/man8/mke2fs.8.html).
