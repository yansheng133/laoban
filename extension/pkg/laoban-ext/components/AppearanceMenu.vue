<script lang="ts">
// Light / auto / dark (Rancher's own user preference) and the laoban colour
// palette, shown in the header of every laoban page. Also loads the global
// palette styles (styles/palettes.scss).
import { defineComponent } from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import { useAppearance } from '../composables/useAppearance';
import { PALETTES, PALETTE_SWATCH, THEMES } from '../config/appearance';

export default defineComponent({
  name: 'LaobanAppearanceMenu',

  setup() {
    const store = useStore();
    const { t } = useText(store);
    const {
      palette, setPalette, themePref, setTheme
    } = useAppearance(store);

    return {
      t, palette, setPalette, themePref, setTheme, PALETTES, PALETTE_SWATCH, THEMES
    };
  },
});
</script>

<template>
  <div
    class="laoban-appearance"
    data-testid="laoban-appearance"
  >
    <div
      class="laoban-appearance-group"
      role="radiogroup"
      :aria-label="t('laoban.appearance.theme')"
    >
      <span class="laoban-appearance-label">{{ t('laoban.appearance.theme') }}</span>
      <button
        v-for="th in THEMES"
        :key="th"
        type="button"
        role="radio"
        class="btn btn-sm"
        :class="themePref === th ? 'role-primary' : 'role-secondary'"
        :aria-checked="themePref === th"
        @click="setTheme(th)"
      >
        {{ t(`laoban.appearance.themes.${ th }`) }}
      </button>
    </div>
    <div
      class="laoban-appearance-group"
      role="radiogroup"
      :aria-label="t('laoban.appearance.palette')"
    >
      <span class="laoban-appearance-label">{{ t('laoban.appearance.palette') }}</span>
      <button
        v-for="p in PALETTES"
        :key="p"
        type="button"
        role="radio"
        class="laoban-swatch"
        :class="{ selected: palette === p }"
        :aria-checked="palette === p"
        :title="t(`laoban.appearance.palettes.${ p }`)"
        @click="setPalette(p)"
      >
        <span
          class="laoban-swatch-dot"
          :style="{ background: PALETTE_SWATCH[p] }"
          aria-hidden="true"
        />
        <span class="laoban-swatch-name">{{ t(`laoban.appearance.palettes.${ p }`) }}</span>
      </button>
    </div>
  </div>
</template>

<style lang="scss">
@import '../styles/palettes.scss';
</style>

<style lang="scss" scoped>
.laoban-appearance {
  display: flex;
  flex-wrap: wrap;
  gap: 8px 16px;
  align-items: center;
  font-size: 12px;
}

.laoban-appearance-group {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  align-items: center;
}

.laoban-appearance-label {
  color: var(--muted);
  margin-right: 4px;
}

.laoban-swatch {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border: 1px solid var(--border);
  border-radius: 12px;
  background: transparent;
  color: var(--body-text);
  cursor: pointer;
  font-size: 12px;

  &.selected {
    border-color: var(--primary);
    box-shadow: 0 0 0 1px var(--primary);
  }

  &:focus-visible {
    outline: 2px solid var(--primary);
    outline-offset: 2px;
  }
}

.laoban-swatch-dot {
  width: 12px;
  height: 12px;
  border-radius: 50%;
  border: 1px solid var(--border);
}
</style>
