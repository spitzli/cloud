# Neon first-party UI snapshot

Canonical source: [spitzli/cloud-plugin-neon](https://github.com/spitzli/cloud-plugin-neon).
The independent repository owns the native runtime, action contracts, release,
documentation and assets. This directory is the Console compatibility UI snapshot;
it introduces no plugin database or alternate provider credentials.

### Reading database details

The overview validates the selected project once before reading its endpoint. Open **Show branches** to fetch the branch list when needed. Read requests have a bounded wait and an explicit retry; they are not retried automatically. Detach remains available after a provider error, but is not shown while the initial overview is pending.
