import { msgid } from '@webdock/i18n';
const labels: Readonly<Record<string,string>> = {
 operator:msgid('Platform operator'),owner:msgid('Owner'),admin:msgid('Tenant admin'),member:msgid('Member'),reader:msgid('Reader'),editor:msgid('Editor'),
 active:msgid('Active'),pending:msgid('Pending'),accepted:msgid('Accepted'),rejected:msgid('Declined'),canceled:msgid('Cancelled'),revoked:msgid('Revoked'),expired:msgid('Expired'),archived:msgid('Archived'),stale:msgid('Outdated'),
 ready:msgid('Ready'),provisioning:msgid('Setting up'),needs_review:msgid('Review required'),verified:msgid('Verified'),missing:msgid('Missing'),conflict:msgid('Conflict'),
 succeeded:msgid('Succeeded'),failed:msgid('Failed'),started:msgid('Started'),denied:msgid('Denied'),production:msgid('Production'),preview:msgid('Preview'),
 monthly:msgid('Monthly'),daily:msgid('Daily'),weekly:msgid('Weekly'),yearly:msgid('Yearly'),
 SEND_SMTP:msgid('SMTP sending'),SEND_API:msgid('API sending'),APIS:msgid('Provider administration'),
};
/** Display labels only. Never feed the result back into an action or stored enum. */
export function authLabel(value:string|null|undefined,t:(source:string)=>string){return value && Object.hasOwn(labels,value)?t(labels[value]):value||'';}
