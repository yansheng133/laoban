// Light/dark (Rancher's own user preference) and the laoban palette.
//
// Light/dark goes through Rancher's prefs store exactly like User
// Preferences does (dispatch 'prefs/setTheme'), so Rancher switches the
// theme class on <body> and remembers it per user. An administrator can
// force a theme with the global "ui-theme" setting; the effective theme is
// then prefs/theme, whatever is chosen here.
//
// The palette is shared by every laoban page (one module-level ref) and
// remembered in localStorage.
import { ref, computed, Ref, ComputedRef } from 'vue';
import {
  loadPalette, savePalette, isPalette, isThemePref, PaletteId, ThemePref
} from '../config/appearance';

interface PrefsStore {
  getters: Record<string, any>;
  dispatch: (type: string, payload?: unknown) => Promise<unknown>;
}

const palette = ref<PaletteId>(loadPalette());

export interface Appearance {
  palette: Ref<PaletteId>;
  setPalette: (id: string) => void;
  themePref: ComputedRef<ThemePref>;
  effectiveTheme: ComputedRef<'light' | 'dark'>;
  setTheme: (value: string) => Promise<void>;
}

export function useAppearance(store: PrefsStore): Appearance {
  const themePref = computed<ThemePref>(() => {
    try {
      const v = store.getters['prefs/get']?.('theme');

      return isThemePref(v) ? v : 'auto';
    } catch {
      return 'auto';
    }
  });

  const effectiveTheme = computed<'light' | 'dark'>(() => (store.getters['prefs/theme'] === 'light' ? 'light' : 'dark'));

  function setPalette(id: string) {
    if (isPalette(id)) {
      palette.value = id;
      savePalette(id);
    }
  }

  async function setTheme(value: string) {
    if (isThemePref(value)) {
      await store.dispatch('prefs/setTheme', value);
    }
  }

  return {
    palette, setPalette, themePref, effectiveTheme, setTheme
  };
}
