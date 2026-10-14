import { describe, it, expect } from '@jest/globals';
import {
  loadPalette, savePalette, isPalette, isThemePref, DEFAULT_PALETTE, PALETTE_STORAGE_KEY, PALETTES
} from '../appearance';

function memory(initial: Record<string, string> = {}) {
  const data = { ...initial };

  return {
    data,
    getItem:  (k: string) => (k in data ? data[k] : null),
    setItem:  (k: string, v: string) => {
      data[k] = v;
    },
  };
}

describe('palette storage', () => {
  it('remembers a chosen palette', () => {
    const s = memory();

    savePalette('ocean', s);
    expect(s.data[PALETTE_STORAGE_KEY]).toBe('ocean');
    expect(loadPalette(s)).toBe('ocean');
  });

  it('falls back to the default for missing or unknown values', () => {
    expect(loadPalette(memory())).toBe(DEFAULT_PALETTE);
    expect(loadPalette(memory({ [PALETTE_STORAGE_KEY]: 'neon' }))).toBe(DEFAULT_PALETTE);
    expect(loadPalette(null)).toBe(DEFAULT_PALETTE);
  });

  it('survives storage that throws', () => {
    const broken = {
      getItem: () => {
        throw new Error('blocked');
      },
      setItem: () => {
        throw new Error('blocked');
      },
    };

    expect(loadPalette(broken)).toBe(DEFAULT_PALETTE);
    expect(() => savePalette('grape', broken)).not.toThrow();
  });

  it('has a default and three more palettes', () => {
    expect(PALETTES).toEqual(['rancher', 'ocean', 'sunset', 'grape']);
    expect(isPalette('sunset')).toBe(true);
    expect(isPalette('Sunset')).toBe(false);
  });

  it("accepts Rancher's theme preference values only", () => {
    expect(['light', 'auto', 'dark'].every(isThemePref)).toBe(true);
    expect(isThemePref('ui-dark')).toBe(false);
  });
});
