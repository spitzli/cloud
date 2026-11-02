# Webdock gettext

English source messages are the fallback. German translations live in real GNU gettext `locales/de/*.po` catalogs. `compiled/de.json` is generated and checked in, so deployments need neither Python nor gettext tools. Runtime code has no PO parser, plural expression evaluator, network request or mutable global language.

## Integration contract

- `@webdock/i18n`: `Locale`, `Preference`, `resolveLocale`, `preferenceFromCookie`, `translator`, `msgid`, `gettext`, `pgettext`, `ngettext`.
- `@webdock/i18n/react`: `I18nProvider({locale,preference,children})` and `useI18n()`.
- `@webdock/i18n/next`: request-cached `getRequestI18n()` and `contextHeaders()`. Import only on the server. The latter returns **only** resolved `Accept-Language`, never auth cookies or credentials.
- `@webdock/i18n/preference`: `createPreferenceResponse(request,{canonicalOrigin})` for an app's POST `/api/locale` handler. Supply trusted app configuration as the origin. It accepts JSON `{preference:"system"|"en"|"de"}` and returns 204 with a host-only, HttpOnly, Secure, SameSite=Lax, year-long cookie. Invalid/duplicate preference cookies resolve to System.
- `@webdock/i18n/picker`: `LanguagePicker` posts that JSON and refreshes React Server Components without changing the URL or reloading the whole document. Wrap it in the app's provider; optional `className` lets existing app styles size it.

`translator(locale)` exposes `{locale,t,n,p,error,date,number}`. `t('Hello {name}',{name})` substitutes named placeholders as **plain text**, without HTML. `n('{count} page','{count} pages',count)` supplies `{count}`. `p('verb','Open')` keeps distinct gettext contexts. EN and DE both use two plural forms (`n != 1`); the compiler rejects other plural rules. Date/number formatting uses en-GB/de-DE; dates default to Europe/Berlin and accept explicit Intl options.

Use literal English messages in translation calls. Mark indirect source labels with `msgid('Source label')`; translate only when rendering. Never translate field keys, action/status enum values, URLs or customer-authored content. The existing CMS `locale` is a content locale and must remain separate from UI language. Unknown messages fall back to their English source. The special `error(message,fallback?)` boundary maps only registered legacy aliases and known catalog messages; unknown provider/database messages become a localized generic error. Add legacy mappings only in `src/legacy-message-aliases.ts`, with English `msgid` values, and include their source messages in the appropriate PO catalog.

Language preferences are host-only. They do not synchronize explicit choices between Studio/Auth/public Webdock or devices. System choice uses weighted supported Accept-Language values. No auth query, signed continuation or identity is rewritten.

## Catalog workflow

Run from this package:

```sh
bun run extract
bun run compile
bun run catalog:check
bun run test
```

The installed GNU xgettext supports TypeScript and TSX directly. Extraction scans an explicit list of workspace source roots; additional CLI paths must stay inside those roots. It writes `locales/webdock.pot`. Markers are `t`, `n`, `p`, `msgid` and the pure gettext functions. Update each owning domain (`auth.po`, `studio.po`, `web.po`, `cms.po`) with reviewed German translations; `core.po` owns the picker and common fallback messages. Avoid merging the entire POT into every domain. Identical duplicate entries are allowed; conflicting translated keys across domains fail compilation. Context is part of a key. Fuzzy/empty translations fall back to source English; their source IDs remain registered for safe error handling.

Compilation runs `msgfmt --check --check-format`, parses its MO result with Python's stdlib `gettext.GNUTranslations`, uses GNU `msgen` identity catalogs to check each source plural form and named placeholders, and emits sorted UTF-8 JSON. No handwritten PO parser or runtime dependency is introduced. Tests also exercise GNU plural/context compilation, merge conflicts and generated-file freshness. GNU gettext and Python are required for development/CI catalog checks only.
