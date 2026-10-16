<script lang="ts">
// "123 / 1000" under a text field; a warning near the limit, and over it
// how much to cut. The text itself is never cut.
import { defineComponent, computed } from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import { textState } from '../utils/limits';

export default defineComponent({
  name: 'LaobanCharCounter',

  props: {
    text: {
      type:    String,
      default: '',
    },
    max: {
      type:     Number,
      required: true,
    },
  },

  setup(props) {
    const store = useStore();
    const { t } = useText(store);
    const s = computed(() => textState(props.text, props.max));

    return { t, s };
  },
});
</script>

<template>
  <p
    class="laoban-counter"
    :class="{ near: s.near && !s.over, over: s.over }"
    aria-live="polite"
    data-testid="laoban-counter"
  >
    <span v-if="s.over">{{ t('laoban.limits.tooLong', { max: s.max, over: s.count - s.max }) }}</span>
    <span class="laoban-counter-count">{{ t('laoban.limits.count', { count: s.count, max: s.max }) }}</span>
  </p>
</template>

<style lang="scss" scoped>
.laoban-counter {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin: 2px 0 0;
  font-size: 12px;
  color: var(--muted);

  &.near {
    color: var(--warning);
  }

  &.over {
    color: var(--error);
    font-weight: 600;
  }
}
</style>
