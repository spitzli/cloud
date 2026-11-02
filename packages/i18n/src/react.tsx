'use client';
import { createContext, createElement, useContext, useMemo, type ReactNode } from 'react';
import { translator, type I18n, type Locale, type Preference } from './core.ts';
export type I18nContext = I18n & { preference: Preference };
const Context = createContext<I18nContext | null>(null);
export function I18nProvider({ locale, preference, children }: { locale: Locale; preference: Preference; children?: ReactNode }) {
 const value = useMemo(() => ({ ...translator(locale), preference }), [locale, preference]);
 return createElement(Context.Provider, { value }, children);
}
export function useI18n(): I18nContext {
 const context = useContext(Context);
 if (!context) throw new Error('useI18n requires an I18nProvider.');
 return context;
}
/** For chrome rendered both inside and before the provider, such as early loading screens. */
export function useOptionalI18n(): I18nContext | null {
 return useContext(Context);
}
