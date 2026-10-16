<script lang="ts">
// "123 / 1000" under a text field; a warning near the limit, and over it
// how much to cut. The text itself is never cut. When the round has less
// room left than the field allows, that is the limit shown (report 20:
// a yellow counter next to a disabled button was confusing).
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
    // Characters the round still has room for.
    left: {
      type:    Number,
      default: Number.POSITIVE_INFINITY,
    },
  },

  setup(props) {
    const store = useStore();
    const { t } = useText(store);
    const byRound = computed(() => props.left < props.max);
    const s = computed(() => textState(props.text, Math.max(0, Math.min(props.max, props.left))));

    return { t, s, byRound };
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
    <span v-if="s.over && byRound">{{ t('laoban.limits.roundOver', { left: s.max }) }}</span>
    <span v-else-if="s.over">{{ t('laoban.limits.tooLong', { max: s.max, over: s.count - s.max }) }}</span>
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
