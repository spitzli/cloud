# Persistent Core jobs queue

`runtime.json` provisions a private Redis instance in `webdock-platform`, separate from the disposable view cache. It requires the namespace quota in `infra/api/kubernetes.yaml` and an operator-created `jobs-redis` Secret with a random `password` value.

Set `WEBDOCK_JOBS_REDIS_URL` in API/worker runtime Secrets to `redis://:<URL-encoded-password>@jobs-redis.webdock-platform.svc.cluster.local:6379/0`. The explicit database path is required by the queue configuration parser. Preserve existing database and encryption keys when updating runtime Secrets.

The queue uses one retained 1 GiB PVC, AOF with `appendfsync everysec`, and `noeviction`. Only API/worker pods can connect. Startup, readiness and liveness probes are required by production admission. The 18 October rollout verified that a test value survived pod recreation, then removed the test value. This single-node queue is persistent but is not highly available and is not a substitute for volume backups.
