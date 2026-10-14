// The 3D view's colour palette (config/appearance.ts). One module-level ref,
// so every 3D view on the page uses the same palette; remembered in
// localStorage.
import { ref, Ref } from 'vue';
import {
  loadPalette, savePalette, isPalette, PaletteId
} from '../config/appearance';

const palette = ref<PaletteId>(loadPalette());

export interface Appearance {
  palette: Ref<PaletteId>;
  setPalette: (id: string) => void;
}

export function useAppearance(): Appearance {
  function setPalette(id: string) {
    if (isPalette(id)) {
      palette.value = id;
      savePalette(id);
    }
  }

  return { palette, setPalette };
}
