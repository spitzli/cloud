import type { I18n } from '@webdock/i18n';
/** Keep action results stable; only their visible wording is localized. */
export function authMessage(message: string, i18n: Pick<I18n, 't' | 'n'>): string {
 const { t, n } = i18n;
 const connection = /^Connection successful\. ([0-9]+) subaccounts? available\.$/.exec(message);
 return connection ? n('Connection successful. {count} subaccount available.', 'Connection successful. {count} subaccounts available.', Number(connection[1])) : t(message);
}
