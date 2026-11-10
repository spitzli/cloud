'use client';

import { useEffect, useRef } from 'react';
import {useI18n} from '@spitzli/i18n/react';

export function ThemePicker() {
  const {t}=useI18n();
  const select = useRef<HTMLSelectElement>(null);
  useEffect(() => {
    const scheme = document.documentElement.dataset.theme;
    if (select.current) select.current.value = scheme === 'light' || scheme === 'dark' ? scheme : 'system';
  }, []);

  return <label className="theme-picker">{t("Appearance")}<select ref={select} defaultValue="system" onChange={(event) => {
      const theme = event.target.value;
      if (theme === 'system') delete document.documentElement.dataset.theme;
      else document.documentElement.dataset.theme = theme;
      try { localStorage.setItem('webdock-theme', theme); } catch { /* The selection still works when storage is blocked. */ }
    }}>
      <option value="system">{t("System")}</option>
      <option value="light">{t("Light")}</option>
      <option value="dark">{t("Dark")}</option>
    </select>
  </label>;
}
