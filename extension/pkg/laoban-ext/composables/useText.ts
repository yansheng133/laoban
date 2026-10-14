// Translated text for laoban's pages.
//
// Rancher's t() HTML-escapes what it returns, for templates that use
// v-html. laoban never does: all text goes through {{ }} or textContent,
// which escape on their own, so an escaped string would show "&quot;" on
// screen, and escaped names would end up in prompts sent to the AI. This
// asks Rancher for the raw string instead.
import { useI18n } from '@shell/composables/useI18n';

export type Translate = (key: string, args?: Record<string, unknown>) => string;

export function useText(store: any): { t: Translate } { // eslint-disable-line @typescript-eslint/no-explicit-any
  const { t } = useI18n(store);

  return { t: (key: string, args?: Record<string, unknown>) => t(key, args, true) };
}
