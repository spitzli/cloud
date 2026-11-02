import { useContext, useEffect, useId, useState, type ReactNode } from "react";
import { CheckIcon } from "@phosphor-icons/react/dist/csr/Check";
import { TranslateIcon } from "@phosphor-icons/react/dist/csr/Translate";
import { I18nProvider, useOptionalI18n } from "@webdock/i18n/react";
import { resolveLocale, type Preference } from "@webdock/i18n";
import { ApiContext } from "./context";
import { alignBelowTrigger, menuPanel } from "./UserMenu";
import { button, cx } from "./ui";
export function LocaleProvider({
  preference,
  children,
}: {
  preference: Preference;
  children: ReactNode;
}) {
  const locale = resolveLocale(navigator.languages.join(","), preference);
  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);
  return (
    <I18nProvider locale={locale} preference={preference}>
      {children}
    </I18nProvider>
  );
}
const languages = [
  { value: "en", label: "English", short: "EN" },
  { value: "de", label: "Deutsch", short: "DE" },
] as const;
/** Menu built on the native Popover API: light dismiss, Escape and focus return come from the browser. */
export function LocalePicker() {
  const api = useContext(ApiContext);
  const i18n = useOptionalI18n();
  const id = useId();
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  if (!api || !i18n) return null;
  const detected = resolveLocale(navigator.languages.join(","), "system");
  const active = languages.find((item) => item.value === i18n.locale) ?? languages[0];
  const choose = async (preference: string) => {
    if (preference === i18n.preference) return;
    setPending(true);
    setFailed(false);
    try {
      const response = await fetch(new URL("/api/locale", api.config.apiOrigin), {
        method: "POST", credentials: "include", redirect: "error",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ preference }),
      });
      if (!response.ok) throw Error("Language preference failed");
      location.reload();
    } catch {
      setFailed(true);
      setPending(false);
    }
  };
  const options = [
    { value: "system", label: i18n.t("System"), detail: languages.find((item) => item.value === detected)?.label ?? detected },
    ...languages.map((item) => ({ value: item.value, label: item.label, detail: "" })),
  ];
  return <div className="relative">
    <button className={cx(`${button("quiet")} gap-1.5 px-2.5 max-[480px]:px-2`)} popoverTarget={id} disabled={pending} aria-label={`${i18n.t("Language")}: ${active.label}`}>
      <TranslateIcon size={18} aria-hidden />
      <span className="text-xs font-semibold tracking-wide max-[480px]:hidden">{active.short}</span>
    </button>
    <div id={id} popover="auto" role="menu" aria-label={i18n.t("Language")}
      onToggle={alignBelowTrigger}
      className={`${menuPanel} w-56`}>
      {options.map((option) => {
        const selected = option.value === i18n.preference;
        return <button key={option.value} role="menuitemradio" aria-checked={selected} disabled={pending}
          className="flex w-full items-center justify-between gap-3 rounded-md px-3 py-2 text-left text-[13px] hover:bg-soft focus-visible:bg-soft focus-visible:outline-none"
          onClick={() => { void choose(option.value); }}>
          <span className="flex flex-col">
            <span className={selected ? "font-semibold" : undefined}>{option.label}</span>
            {option.detail ? <span className="text-xs text-muted">{option.detail}</span> : null}
          </span>
          {selected ? <CheckIcon size={16} className="text-accent" aria-hidden /> : null}
        </button>;
      })}
      {failed ? <p role="alert" className="px-3 py-2 text-xs text-danger">{i18n.t("Something went wrong. Please try again.")}</p> : null}
    </div>
  </div>;
}
