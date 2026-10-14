<script lang="ts">
// The plan returned by /api/plan (summary, steps, usage), the "confirm and
// deploy" button, and after /api/apply the commit link and, once the new
// version runs, the app link. Shared by the new-app and app detail pages.
//
// AI text is rendered with {{ }} only, never v-html.
import { defineComponent, PropType } from 'vue';
import { useStore } from 'vuex';
import { useI18n } from '@shell/composables/useI18n';
import type { PlanResponse, ApplyResponse } from '../utils/backend';

export default defineComponent({
  name: 'LaobanPlanResult',

  props: {
    plan:        { type: Object as PropType<PlanResponse | null>, default: null },
    steps:       { type: Array as PropType<string[]>, default: () => [] },
    applied:     { type: Object as PropType<ApplyResponse | null>, default: null },
    phase:       { type: String, default: 'idle' },
    canApply:    { type: Boolean, default: false },
    commitLink:  { type: String as PropType<string | null>, default: null },
    shortCommit: { type: String, default: '' },
    appLink:     { type: String as PropType<string | null>, default: null },
  },

  emits: ['apply'],

  setup() {
    const store = useStore();
    const { t } = useI18n(store);
    const formatNumber = (n: unknown) => (typeof n === 'number' && Number.isFinite(n) ? n.toLocaleString() : '-');

    return { t, formatNumber };
  },
});
</script>

<template>
  <div>
    <div
      v-if="plan"
      class="laoban-plan"
      data-testid="laoban-plan"
    >
      <h3>{{ plan.mode === 'edit' ? t('laoban.edit.planTitle', { appName: plan.appName }) : t('laoban.dashboard.plan.title', { appName: plan.appName }) }}</h3>
      <p class="laoban-text">
        {{ plan.summary }}
      </p>
      <h4>{{ t('laoban.dashboard.plan.steps') }}</h4>
      <ol>
        <li
          v-for="(step, i) in steps"
          :key="i"
          class="laoban-text"
        >
          {{ step }}
        </li>
      </ol>
      <p
        v-if="plan.maskedFindings"
        class="text-muted"
      >
        {{ t('laoban.dashboard.plan.masked', { count: plan.maskedFindings }) }}
      </p>
      <p
        v-if="plan.usage"
        class="text-muted"
      >
        {{ t('laoban.dashboard.plan.usage', { input: formatNumber(plan.usage.inputTokens), output: formatNumber(plan.usage.outputTokens) }) }}
      </p>
      <button
        type="button"
        class="btn role-primary"
        :disabled="!canApply"
        @click="$emit('apply')"
      >
        <i
          v-if="phase === 'applying'"
          class="icon icon-spinner icon-spin"
          aria-hidden="true"
        />
        {{ t('laoban.dashboard.plan.apply') }}
      </button>
    </div>

    <div
      v-if="applied"
      class="laoban-result"
      data-testid="laoban-result"
    >
      <p>
        {{ t('laoban.dashboard.result.committed') }}
        <a
          v-if="commitLink"
          :href="commitLink"
          target="_blank"
          rel="noopener noreferrer"
        >{{ shortCommit }}</a>
        <span v-else>{{ shortCommit }}</span>
      </p>
      <p
        v-if="phase === 'ready'"
        class="laoban-result-actions"
      >
        <a
          v-if="appLink"
          :href="appLink"
          target="_blank"
          rel="noopener noreferrer"
          class="btn role-primary"
        >{{ t('laoban.dashboard.result.openApp') }}</a>
        <span
          v-else
          class="text-error"
        >{{ t('laoban.dashboard.result.badAppUrl') }}</span>
        <slot name="ready" />
      </p>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.laoban-plan,
.laoban-result {
  margin-top: 16px;
  padding-top: 12px;
  border-top: 1px solid var(--border);

  ol {
    padding-left: 20px;
    margin-bottom: 12px;
  }
}

.laoban-result-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  align-items: center;
}

.laoban-text {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
</style>
