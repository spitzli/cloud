import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
// Retained TypeScript Mail contract; this does not establish native Rust endpoint parity.
import { mailOpenAPI } from '../../legacy/control-plane/src/modules/mail/product/openapi';

const target = new URL('../openapi/mail.json', import.meta.url);
const spec = mailOpenAPI();
// Hosted docs use the canonical API host; runtime specs stay relative for local testing.
spec.servers = [{ url: 'https://api.spitzli.cloud/api/mail/v1' }];
const titles: Record<string, string> = {
  emails_list: 'List emails', emails_send: 'Send an email',
  emails_batch: 'Send a batch', emails_get: 'Retrieve an email',
  emails_reschedule: 'Reschedule an email', emails_cancel: 'Cancel an email',
  batches_get: 'Retrieve a batch',
  domains_list: 'List domains', domains_create: 'Add a domain',
  domains_get: 'Retrieve a domain', domains_delete: 'Remove a domain',
  domains_verify: 'Verify a domain',
  keys_list: 'List API keys', keys_create: 'Create an API key', keys_delete: 'Revoke an API key',
  webhooks_list: 'List webhooks', webhooks_create: 'Create a webhook',
  webhooks_get: 'Retrieve a webhook', webhooks_update: 'Update a webhook',
  webhooks_delete: 'Delete a webhook', webhooks_deliveries: 'List webhook deliveries',
  webhooks_rotate: 'Rotate a signing secret', webhooks_replay: 'Replay a delivery',
  suppressions_list: 'List suppressions', suppressions_create: 'Add a suppression',
  suppressions_delete: 'Remove a suppression',
  logs_list: 'List request logs', logs_get: 'Retrieve a request log',
  analytics_get: 'Retrieve analytics',
};
const groups: Record<string, string> = {
  emails: 'Emails', batches: 'Batches', domains: 'Domains', keys: 'API keys',
  webhooks: 'Webhooks', suppressions: 'Suppressions', logs: 'Logs', analytics: 'Analytics',
};
// Presentation only: retain operation IDs, paths, auth, parameters and schemas.
for (const path of Object.values(spec.paths)) {
  for (const value of Object.values(path)) {
    const operation = value as { operationId: string; summary: string; tags?: string[] };
    const group = operation.operationId.split('_')[0]!;
    operation.summary = titles[operation.operationId] ?? operation.summary;
    operation.tags = [groups[group] ?? group];
  }
}
const output = JSON.stringify(spec, null, 2) + '\n';

if (process.argv.includes('--check')) {
  if (readFileSync(target, 'utf8') !== output) {
    throw new Error('Mail OpenAPI snapshot is stale. Run bun docs/scripts/export-mail-openapi.ts');
  }
  console.log('Mail OpenAPI snapshot matches the application contract.');
} else {
  mkdirSync(fileURLToPath(new URL('../openapi/', import.meta.url)), { recursive: true });
  writeFileSync(target, output);
  console.log('Updated docs/openapi/mail.json');
}
