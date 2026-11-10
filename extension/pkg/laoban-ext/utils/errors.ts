// Words for an error, in the page's language where laoban knows the code
// (the backend writes its own messages in Chinese; report 21, N8).
import { RequestError } from './backend';

type Translate = (key: string, args?: Record<string, unknown>) => string;

// Error codes the page words itself (laoban.errors.*).
export const TRANSLATED_ERRORS = [
  'forbidden', 'unauthenticated', 'unavailable', 'backend_not_found', 'version_conflict', 'not_initiator', 'not_confirmer',
  'round_state', 'budget_exceeded', 'spend_limit', 'github_failed', 'round_not_found', 'busy', 'app_not_found',
  'version_not_found', 'round_active', 'app_exists',
  // Report 26, N44: shown in English on a Chinese page before.
  'ai_invalid_output', 'plan_not_found', 'already_applied',
  // i18n review (2026-11): every other code the backend sends, so a page
  // never shows the backend's install-time language instead of its own.
  'plan_store_failed', 'internal', 'round_scope', 'merge_conflict', 'too_long', 'round_full', 'opinion_not_found',
  'invalid_files', 'job_not_found', 'rounds_unavailable', 'round_conflict', 'state_failed', 'bad_request'
];

// Codes whose backend message names the exact cause (which file failed a
// check, which limit was hit). The page shows its own words and keeps the
// backend's message after them, so the detail is not lost when the backend
// was installed in another language.
export const DETAILED_ERRORS = ['invalid_files', 'bad_request'];

/** The page-language text for a known error code, or '' when there is none. */
export function translatedError(t: Translate, e: unknown): string {
  if (!(e instanceof RequestError) || !TRANSLATED_ERRORS.includes(e.code)) {
    return '';
  }
  const text = t(`laoban.errors.${ e.code }`);

  if (DETAILED_ERRORS.includes(e.code) && e.userMessage) {
    return `${ text }${ t('laoban.common.paren', { text: e.userMessage }) }`;
  }

  return text;
}

export function errorText(t: Translate, e: unknown, fallbackKey: string): string {
  const known = translatedError(t, e);

  if (known) {
    return known;
  }
  if (e instanceof RequestError) {
    if (e.userMessage) {
      return e.userMessage;
    }
    if (e.code === 'timeout') {
      return t('laoban.dashboard.errors.timeout');
    }
  }

  return t(fallbackKey);
}
