<script lang="ts">
// What the subagents writing the code are doing: one card for the web part
// and one for the API part, written at the same time from the same plan.
// Shown while a plan is applied (from the job) and while a round applies
// (from the round), and kept after it is done with what each one used.
import { computed, defineComponent, PropType } from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import { ContractGap, SubagentRun } from '../utils/backend';

export default defineComponent({
  name: 'LaobanSubagentPanel',

  props: {
    runs:    { type: Array as PropType<SubagentRun[]>, default: () => [] },
    gaps:    { type: Array as PropType<ContractGap[]>, default: () => [] },
    // Seconds since the code writing started, counted by the page: the
    // backend reports a running subagent's seconds only when it ends.
    elapsed: { type: Number, default: 0 },
  },

  setup(props) {
    const store = useStore();
    const { t } = useText(store);

    const formatNumber = (n: unknown) => (typeof n === 'number' && Number.isFinite(n) ? n.toLocaleString() : '-');
    const fileNames = (files: string[]) => files.map((f) => f.split('/').pop()).join(t('laoban.common.listSep'));
    const running = computed(() => props.runs.some((r) => r.state === 'running'));
    const written = computed(() => props.runs.filter((r) => r.state !== 'skipped').length);

    function stateText(r: SubagentRun): string {
      switch (r.state) {
      case 'running':
        return t('laoban.subagents.state.running', { seconds: props.elapsed });
      case 'done':
        return t('laoban.subagents.state.done', { seconds: r.seconds });
      case 'failed':
        return t('laoban.subagents.state.failed', { seconds: r.seconds });
      default:
        return t('laoban.subagents.state.skipped');
      }
    }

    return {
      t, formatNumber, fileNames, running, written, stateText
    };
  },
});
</script>

<template>
  <section
    v-if="runs.length"
    class="laoban-subagents"
    data-testid="laoban-subagents"
    :aria-label="t('laoban.subagents.title')"
  >
    <h3>{{ t('laoban.subagents.title') }}</h3>
    <p class="text-muted laoban-subagents-hint">
      {{ written > 1 ? t('laoban.subagents.hintBoth') : t('laoban.subagents.hintOne') }}
    </p>
    <ul class="laoban-subagent-list">
      <li
        v-for="r in runs"
        :key="r.part"
        class="laoban-subagent"
        :class="`state-${ r.state }`"
        :data-testid="`laoban-subagent-${ r.part }`"
      >
        <div class="laoban-subagent-head">
          <i
            v-if="r.state === 'running'"
            class="icon icon-spinner icon-spin"
            aria-hidden="true"
          />
          <i
            v-else-if="r.state === 'done'"
            class="icon icon-checkmark text-success"
            aria-hidden="true"
          />
          <i
            v-else-if="r.state === 'failed'"
            class="icon icon-error text-error"
            aria-hidden="true"
          />
          <i
            v-else
            class="icon icon-minus text-muted"
            aria-hidden="true"
          />
          <strong>{{ t(`laoban.subagents.part.${ r.part }`) }}</strong>
        </div>
        <div class="laoban-subagent-files text-muted">
          {{ fileNames(r.files) }}
        </div>
        <div
          class="laoban-subagent-state"
          role="status"
        >
          {{ stateText(r) }}
        </div>
        <div
          v-if="r.fixed"
          class="laoban-subagent-fixed text-muted"
        >
          {{ t('laoban.subagents.fixed') }}
        </div>
        <div
          v-if="r.advisorModel"
          class="laoban-subagent-fixed text-muted"
        >
          {{ t('laoban.subagents.advisor', { model: r.advisorModel, tokens: formatNumber(r.advisorTokens) }) }}
        </div>
        <div
          v-if="r.state === 'done'"
          class="laoban-subagent-usage text-muted"
        >
          {{ t('laoban.subagents.usage', { input: formatNumber(r.inputTokens), output: formatNumber(r.outputTokens) }) }}
        </div>
      </li>
    </ul>
    <div
      v-if="gaps.length && !running"
      class="laoban-subagent-gaps"
      role="note"
    >
      <p class="text-warning">
        {{ t('laoban.subagents.gapsTitle') }}
      </p>
      <ul>
        <li
          v-for="g in gaps"
          :key="`${ g.kind }:${ g.method }:${ g.path }`"
        >
          {{ t(`laoban.subagents.gap.${ g.kind }`, { path: `${ g.method || '' } ${ g.path }`.trim() }) }}
        </li>
      </ul>
      <p class="text-muted">
        {{ t('laoban.subagents.gapsHint') }}
      </p>
    </div>
  </section>
</template>

<style lang="scss" scoped>
.laoban-subagents {
  margin: 12px 0;

  h3 {
    margin: 0 0 4px;
    font-size: 15px;
  }
}

.laoban-subagents-hint {
  margin: 0 0 8px;
  font-size: 13px;
}

.laoban-subagent-list {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  gap: 8px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.laoban-subagent {
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: var(--border-radius);
  background: var(--body-bg);
  font-size: 13px;
  min-width: 0;

  &.state-running {
    border-color: var(--primary);
  }

  &.state-failed {
    border-color: var(--error);
  }

  &.state-skipped {
    opacity: 0.75;
  }
}

.laoban-subagent-head {
  display: flex;
  align-items: center;
  gap: 6px;
}

.laoban-subagent-files,
.laoban-subagent-fixed,
.laoban-subagent-usage {
  overflow-wrap: anywhere;
  font-size: 12px;
}

.laoban-subagent-state {
  margin-top: 4px;
}

.laoban-subagent-gaps {
  margin-top: 8px;
  font-size: 13px;

  p {
    margin: 4px 0;
  }

  ul {
    margin: 0;
    padding-left: 20px;
  }

  code {
    overflow-wrap: anywhere;
  }
}
</style>
