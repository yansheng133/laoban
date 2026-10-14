import { describe, it, expect } from '@jest/globals';
import { uiLocale, formatClock, formatDateTime } from '../time';

const storeFor = (locale: string) => ({ getters: { 'i18n/current': () => locale } });

describe('UI locale for dates', () => {
  it('maps Rancher locales', () => {
    expect(uiLocale(storeFor('ja'))).toBe('ja-JP');
    expect(uiLocale(storeFor('en-us'))).toBe('en-US');
    expect(uiLocale(storeFor('zh-hant'))).toBe('zh-TW');
    expect(uiLocale(storeFor('xx'))).toBeUndefined();
    expect(uiLocale({ getters: {} })).toBeUndefined();
  });

  it('formats in that locale and keeps bad input', () => {
    const at = '2026-10-14T10:30:00Z';

    expect(formatDateTime(storeFor('en-us'), at)).toBe(new Date(at).toLocaleString('en-US'));
    expect(formatClock(storeFor('ja'), at)).toBe(new Date(at).toLocaleTimeString('ja-JP'));
    expect(formatDateTime(storeFor('ja'), 'not a date')).toBe('not a date');
  });
});
