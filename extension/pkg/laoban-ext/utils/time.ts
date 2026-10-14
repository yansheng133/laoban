// Dates and times in the language the UI is shown in (Rancher's locale),
// not the browser's.

interface StoreWithGetters {
  getters: Record<string, unknown>;
}

const LOCALES: Record<string, string> = {
  'zh-hant': 'zh-TW',
  'en-us':   'en-US',
  ja:        'ja-JP',
};

export function uiLocale(store: StoreWithGetters): string | undefined {
  try {
    const current = store.getters['i18n/current'];
    const id = typeof current === 'function' ? String(current()) : '';

    return LOCALES[id];
  } catch {
    return undefined;
  }
}

export function formatDateTime(store: StoreWithGetters, at: string | number | Date): string {
  const d = new Date(at);

  return Number.isNaN(d.getTime()) ? String(at) : d.toLocaleString(uiLocale(store));
}

export function formatClock(store: StoreWithGetters, at: string | number | Date): string {
  const d = new Date(at);

  return Number.isNaN(d.getTime()) ? String(at) : d.toLocaleTimeString(uiLocale(store));
}
