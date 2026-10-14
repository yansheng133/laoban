// Colour palettes for the laoban pages, on top of Rancher's own light/dark
// theme. "rancher" keeps Rancher's colours; the others override Rancher's
// primary/link variables inside the laoban pages only, and the 3D view's
// layer colours. The CSS for each lives in styles/palettes.scss.
//
// The chosen palette is remembered per browser (localStorage). Light/dark
// is Rancher's own user preference (prefs/theme), not stored here.

export const PALETTES = ['rancher', 'ocean', 'sunset', 'grape'] as const;

export type PaletteId = typeof PALETTES[number];

export const DEFAULT_PALETTE: PaletteId = 'rancher';

export const PALETTE_STORAGE_KEY = 'laoban.palette';

// Swatch shown in the picker (light-theme primary of each palette).
export const PALETTE_SWATCH: Record<PaletteId, string> = {
  rancher: 'var(--primary)',
  ocean:   '#1f6fb2',
  sunset:  '#b04e14',
  grape:   '#6a4bc4',
};

export function isPalette(v: unknown): v is PaletteId {
  return typeof v === 'string' && (PALETTES as readonly string[]).includes(v);
}

interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function storage(): StorageLike | null {
  try {
    return typeof window !== 'undefined' && window.localStorage ? window.localStorage : null;
  } catch {
    return null;
  }
}

/** The remembered palette, or the default if none or unreadable. */
export function loadPalette(store: StorageLike | null = storage()): PaletteId {
  try {
    const v = store?.getItem(PALETTE_STORAGE_KEY);

    return isPalette(v) ? v : DEFAULT_PALETTE;
  } catch {
    return DEFAULT_PALETTE;
  }
}

/** Remember the palette; failures (private mode, blocked storage) are ignored. */
export function savePalette(id: PaletteId, store: StorageLike | null = storage()): void {
  try {
    store?.setItem(PALETTE_STORAGE_KEY, id);
  } catch {
    // Not remembered; the page still uses it until reload.
  }
}

// Rancher's theme preference values (store/prefs.js THEME options).
export const THEMES = ['light', 'auto', 'dark'] as const;

export type ThemePref = typeof THEMES[number];

export function isThemePref(v: unknown): v is ThemePref {
  return typeof v === 'string' && (THEMES as readonly string[]).includes(v);
}
