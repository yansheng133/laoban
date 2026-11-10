// Error words in the page's language, whatever language the backend was
// installed with (i18n review 2026-11).
import { describe, it, expect } from '@jest/globals';
import { errorText, translatedError } from '../errors';
import { RequestError } from '../backend';

const t = (key: string, args?: Record<string, unknown>) => (args ? `${ key } ${ JSON.stringify(args) }` : key);

describe('errors', () => {
  it('words a known code itself instead of the backend message', () => {
    const e = new RequestError('round_full', 'This round is full.', 400);

    expect(errorText(t, e, 'fallback')).toBe('laoban.errors.round_full');
  });

  it('keeps the backend detail after its own words for detailed codes', () => {
    const e = new RequestError('invalid_files', 'handler.py must define handle(method, path, body).', 400);

    expect(translatedError(t, e)).toBe('laoban.errors.invalid_fileslaoban.common.paren {"text":"handler.py must define handle(method, path, body)."}');
  });

  it('falls back to the backend message, then to the fallback key', () => {
    expect(errorText(t, new RequestError('something_new', 'Backend words.', 400), 'fallback')).toBe('Backend words.');
    expect(errorText(t, new RequestError('something_new', null, 500), 'fallback')).toBe('fallback');
    expect(translatedError(t, new Error('x'))).toBe('');
  });
});
