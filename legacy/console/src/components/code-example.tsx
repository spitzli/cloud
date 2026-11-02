'use client';
import { useState } from 'react';
import { useI18n } from '@webdock/i18n/react';

export function CodeExample({ label, code }: { label: string; code: string }) {
  const { t } = useI18n();
  const [status, setStatus] = useState('');
  return <div className="code-example">
    <div><strong>{label}</strong><button type="button" className="button secondary" onClick={async () => {
      try { await navigator.clipboard.writeText(code); setStatus(t('Copied')); }
      catch { setStatus(t('Copy failed. Select and copy the example.')); }
    }}>{t('Copy')}</button></div>
    <pre tabIndex={0}><code>{code}</code></pre>
    <span role="status">{status}</span>
  </div>;
}
