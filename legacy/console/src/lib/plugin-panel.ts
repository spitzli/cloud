/** Resolve only a panel served by the installed extension's configured HTTPS origin. */
export function pluginPanelURL(origin: string | undefined, path: string, local: {allowLocalHTTP?: boolean; environment?: string} = {}): string | null {
  if (!origin || !path.startsWith('/') || path.startsWith('//') || path.includes('\\')) return null;
  try {
    const base = new URL(origin);
    const panel = new URL(path, base);
    const developmentLoopback = local.allowLocalHTTP === true && (local.environment ?? process.env.NODE_ENV) !== 'production' && base.protocol === 'http:' && ['localhost','127.0.0.1','[::1]'].includes(base.hostname);
    if ((base.protocol !== 'https:' && !developmentLoopback) || base.username || base.password || panel.origin !== base.origin) return null;
    return panel.href;
  } catch { return null; }
}
