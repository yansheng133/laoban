// Colour palettes of the 3D view, chosen with its 🎨 button: the layer
// colours and the colour of the selected component. The page itself keeps
// Rancher's colours, and light/dark is Rancher's own user preference. The
// CSS for each palette lives in styles/palettes.scss.
//
// The chosen palette is remembered per browser (localStorage), under the
// same key as before, so an earlier choice carries over.

export const PALETTES = ['rancher', 'ocean', 'sunset', 'grape'] as const;

export type PaletteId = typeof PALETTES[number];

export const DEFAULT_PALETTE: PaletteId = 'rancher';

export const PALETTE_STORAGE_KEY = 'laoban.palette';

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
