<script lang="ts">
// Bottom status line of the laoban pages: what the deploy flow is doing.
import { defineComponent } from 'vue';
import { useStore } from 'vuex';
import { useI18n } from '@shell/composables/useI18n';

export default defineComponent({
  name: 'LaobanStatusBar',

  props: {
    text:      { type: String, default: '' },
    spinning:  { type: Boolean, default: false },
    // Fleet's own message (text from the cluster; interpolated, never v-html).
    message:   { type: String, default: '' },
    pollError: { type: Boolean, default: false },
  },

  setup() {
    const store = useStore();
    const { t } = useI18n(store);

    return { t };
  },
});
</script>

<template>
  <footer
    class="laoban-status"
    role="status"
    aria-live="polite"
  >
    <i
      v-if="spinning"
      class="icon icon-spinner icon-spin"
      aria-hidden="true"
    />
    <span>{{ text }}</span>
    <span
      v-if="message"
      class="text-muted laoban-text"
    >{{ message }}</span>
    <span
      v-if="pollError"
      class="text-warning"
    >{{ t('laoban.dashboard.status.pollFailed') }}</span>
  </footer>
</template>

<style lang="scss" scoped>
.laoban-status {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px 12px;
  padding: 8px 12px;
  border: 1px solid var(--border);
  border-radius: var(--border-radius);
  background: var(--body-bg);
  font-size: 13px;
}

.laoban-text {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
</style>
