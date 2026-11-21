# Development handoff · 21 November 2026

This development branch contains the local agent transport admission change, the retained migrations through schema 79 and the follow-up waitlist migration. The agent reads current enrollment and retained authority together and checks the configured database authority before serving. The waitlist migration preserves the deployed migration bytes and serializes initial budget-row creation. Migration 80 belongs to the separate CMS execution work. Keep its version reserved when bringing that work into this branch.

The Website and CMS instance use Next.js 16.4.6 in their workspace dependency graph. Core uses rmcp 3.9.0 and the mail library uses lettre 0.11.24. The workspace also updates Nodemailer, DOMPurify, Zod, Tokio and TLS dependencies. The dependency scans still report three Rust advisories and fifteen JavaScript advisories. Check the remaining dependency paths against the release artifacts before promotion.

To validate an isolated development environment, run the standard quality workflow on Blacksmith. The waitlist regression runs with `python3 tools/tests/waitlist-budget.py`. For the transport regression, build the migrator with `cargo build --locked -p spitzli-control-plane --bin spitzli-migrate` and run `python3 tools/tests/agent-transport-authority.py`. Both scripts create disposable PostgreSQL fixtures on the local Docker daemon.

Before promoting a release, qualify the retained database upgrade and fallback with the exact artifact. Operational release gates include independent backup-key recovery, customer data restore and confirmed alert delivery.
