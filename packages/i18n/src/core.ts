import german from '../compiled/de.json' with { type: 'json' };
import { legacyMessageAliases } from './legacy-message-aliases.ts';
import { msgid } from './marker.ts';
export { msgid } from './marker.ts';
export type Locale = 'en' | 'de';
export type Preference = 'system' | Locale;
export type Params = Readonly<Record<string, string | number | boolean | null | undefined>>;
export const LOCALE_COOKIE = 'webdock_locale';
export const GENERIC_ERROR = msgid('Something went wrong. Please try again.');
const catalog: Readonly<Record<string, readonly string[]>> = german;
export const isPreference = (value: unknown): value is Preference => value === 'system' || value === 'en' || value === 'de';
export function preferenceFromCookie(cookie: string | null | undefined): Preference {
 const values = (cookie || '').split(';').map(part => part.trim()).filter(part => part.startsWith(LOCALE_COOKIE + '='));
 if (values.length !== 1) return 'system';
 const value = values[0]?.slice(LOCALE_COOKIE.length + 1);
 return isPreference(value) ? value : 'system';
}
export function resolveLocale(acceptLanguage: string | null, preference?: string | null): Locale {
 if (preference === 'en' || preference === 'de') return preference;
 const candidates = (acceptLanguage || '').slice(0, 8192).split(',').map((part, index) => {
  const match = part.trim().match(/^([a-z]{1,8}(?:-[a-z0-9]{1,8})*|\*)(?:\s*;\s*q=(0(?:\.\d{0,3})?|1(?:\.0{0,3})?))?$/i);
  return match?.[1] ? { language: match[1].toLowerCase().split('-')[0], quality: match[2] === undefined ? 1 : Number(match[2]), index } : null;
 }).filter((candidate): candidate is NonNullable<typeof candidate> => candidate !== null && candidate.quality > 0).sort((a, b) => b.quality - a.quality || a.index - b.index);
 for (const candidate of candidates) {
  if (candidate.language === 'de') return 'de';
  if (candidate.language === 'en' || candidate.language === '*') return 'en';
 }
 return 'en';
}
function interpolate(value: string, params: Params = {}): string {
 return value.replace(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g, (placeholder, key: string) => Object.hasOwn(params, key) ? String(params[key] ?? '') : placeholder);
}
function lookup(locale: Locale, key: string, fallback: string, plural = 0): string {
 if (locale !== 'de' || !Object.hasOwn(catalog, key)) return fallback;
 return catalog[key]?.[plural] || fallback;
}
/** Translations are plain text. React escapes them; HTML callers must do so too. */
export function gettext(locale: Locale, source: string, params?: Params): string {
 return interpolate(lookup(locale, source, source), params);
}
export function pgettext(locale: Locale, context: string, source: string, params?: Params): string {
 return interpolate(lookup(locale, context + '\u0004' + source, source), params);
}
export function ngettext(locale: Locale, singular: string, plural: string, count: number, params?: Params): string {
 if (!Number.isFinite(count)) throw new RangeError('Plural count must be finite.');
 const index = count === 1 ? 0 : 1;
 return interpolate(lookup(locale, singular, index ? plural : singular, index), { ...params, count });
}
function knownMessage(message: string): string | null {
 const source = Object.hasOwn(legacyMessageAliases, message) ? (legacyMessageAliases[message] ?? message) : message;
 return Object.hasOwn(catalog, source) ? source : null;
}
export function translator(locale: Locale) {
 const intlLocale = locale === 'de' ? 'de-DE' : 'en-GB';
 return {
  locale,
  t: (source: string, params?: Params) => gettext(locale, source, params),
  n: (singular: string, plural: string, count: number, params?: Params) => ngettext(locale, singular, plural, count, params),
  p: (context: string, source: string, params?: Params) => pgettext(locale, context, source, params),
  error: (message: unknown, fallback: string = GENERIC_ERROR) => gettext(locale, (typeof message === 'string' ? knownMessage(message) : null) || knownMessage(fallback) || GENERIC_ERROR),
  date: (value: string | number | Date, options?: Intl.DateTimeFormatOptions) => new Intl.DateTimeFormat(intlLocale, { timeZone: 'Europe/Berlin', ...(options || { dateStyle: 'medium' }) }).format(value instanceof Date ? value : new Date(value)),
  number: (value: number, options?: Intl.NumberFormatOptions) => new Intl.NumberFormat(intlLocale, options).format(value),
 };
}
export type I18n = ReturnType<typeof translator>;
