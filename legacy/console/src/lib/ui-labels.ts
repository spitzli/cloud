import { msgid } from '@webdock/i18n';
// Presentation only. Stored enums and action identifiers are unchanged.
const statuses: Record<string,string> = {
 running:msgid("In progress"), stopped:msgid("Stopped"), "needs-reconciliation":msgid("Review required"),
 started:msgid("Started"), succeeded:msgid("Successful"), "create-organization":msgid("Create customer group"), "link-website":msgid("Assign website"), invite:msgid("Invite person"), "change-access":msgid("Change website access"), "revoke-access":msgid("Revoke website access"), suspend:msgid("Suspend account"), "cancel-invite":msgid("Cancel invitation"),
 active:msgid('Active'), archived:msgid('Archived'), pending:msgid('Pending'), accepted:msgid('Accepted'), cancelled:msgid('Cancelled'), expired:msgid('Expired'), suspended:msgid('Suspended'), retired:msgid('Retired'), enabled:msgid('Enabled'), disabled:msgid('Disabled'), revoked:msgid('Revoked'),
 ready:msgid('Ready'), building:msgid('Building'), queued:msgid('Queued'), initializing:msgid('Initializing'), error:msgid('Error'), canceled:msgid('Cancelled'), verified:msgid('Verified'), draft:msgid('Draft'), published:msgid('Published'), reviewed:msgid('Reviewed'),
 operator:msgid('Superadmin'), admin:msgid('Tenant admin'), owner:msgid('Owner'), member:msgid('Member'), editor:msgid('Editor'), reader:msgid('Read-only'), user:msgid('User'),
 provisioning:msgid('Provisioning'), reconciliation:msgid('Review required'), pending_review:msgid('Review required'), success:msgid('Successful'), failed:msgid('Failed'), failure:msgid('Failed'), allowed:msgid('Allowed'), denied:msgid('Denied'),
 create:msgid('Created'), update:msgid('Updated'), archive:msgid('Archived'), restore:msgid('Restored'), project:msgid('Project'), customer:msgid('Customer'), 'cms-instance':msgid('CMS connection'),
};
export function uiLabel(value: string) { return statuses[value.toLowerCase()] || value; }

const auditFields: Record<string,string> = {
 name:msgid('Name'),label:msgid('Label'),status:msgid('Status'),customer:msgid('Customer'),project:msgid('Project'),url:msgid('Website URL'),repositoryURL:msgid('Repository URL'),notes:msgid('Notes'),adminURL:msgid('CMS URL'),schemaName:msgid('Database schema'),providerProjectID:msgid('Hosting project ID'),provider:msgid('Hosting provider'),template:msgid('Template'),payloadVersion:msgid('CMS engine version'),vercelConnection:msgid('Vercel connection'),
 customerType:msgid('Customer type'),companyName:msgid('Company name'),firstName:msgid('First name'),lastName:msgid('Last name'),contactName:msgid('Contact person'),contactEmail:msgid('Contact email'),phone:msgid('Phone'),addressLine1:msgid('Street and number'),addressLine2:msgid('Address addition'),postalCode:msgid('Postal code'),city:msgid('City'),region:msgid('State / region'),country:msgid('Country code'),
};
export function auditFieldLabel(value: string) { return auditFields[value.trim()] || value.trim(); }
export function auditSummary(summary: string, t: (message: string, params?: Record<string,string>) => string) {
  if (summary.startsWith('Created ')) return t('Created {name}', {name:summary.slice(8)});
  if (summary.startsWith('Updated ')) return t('Updated {name}', {name:summary.slice(8)});
  const known: string[] = [msgid('Connected read-only Vercel integration'),msgid('Connected Vercel integration'),msgid('Vercel project region set to fra1; automatic regional failover disabled.'),msgid('Vercel project region already fra1; automatic regional failover disabled.'),msgid('Disconnected Vercel from Studio'),msgid('Linked Vercel hosting project')];
  return known.includes(summary) ? t(summary) : summary;
}
