// Words for an error, in the page's language where laoban knows the code
// (the backend writes its own messages in Chinese; report 21, N8).
import { RequestError } from './backend';

type Translate = (key: string, args?: Record<string, unknown>) => string;

// Error codes the page words itself (laoban.errors.*).
export const TRANSLATED_ERRORS = [
  'forbidden', 'unauthenticated', 'unavailable', 'backend_not_found', 'version_conflict', 'not_initiator', 'not_confirmer',
  'round_state', 'budget_exceeded', 'spend_limit', 'github_failed', 'round_not_found', 'busy', 'app_not_found',
  'version_not_found', 'round_active', 'app_exists'
];

export function errorText(t: Translate, e: unknown, fallbackKey: string): string {
  if (e instanceof RequestError) {
    if (TRANSLATED_ERRORS.includes(e.code)) {
      return t(`laoban.errors.${ e.code }`);
    }
    if (e.userMessage) {
      return e.userMessage;
    }
    if (e.code === 'timeout') {
      return t('laoban.dashboard.errors.timeout');
    }
  }

  return t(fallbackKey);
}
