<script lang="ts">
// The plan's API contract: each endpoint with the request and response
// examples both subagents write from. Shown where a plan is confirmed, so a
// person sees the shapes before the code is written (review H3).
import { defineComponent, PropType } from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import type { PlanEndpoint } from '../utils/backend';

export default defineComponent({
  name: 'LaobanContractTable',

  props: {
    endpoints: { type: Array as PropType<PlanEndpoint[]>, default: () => [] },
  },

  setup() {
    const store = useStore();
    const { t } = useText(store);

    // Examples arrive as compact JSON; shown indented. Anything else as is.
    function pretty(text?: string): string {
      if (!text) {
        return '';
      }
      try {
        return JSON.stringify(JSON.parse(text), null, 2);
      } catch (e) {
        return text;
      }
    }

    return { t, pretty };
  },
});
</script>

<template>
  <details
    v-if="endpoints.length"
    class="laoban-contract"
    data-testid="laoban-contract"
  >
    <summary>{{ t('laoban.contract.title', { count: endpoints.length }) }}</summary>
    <p class="text-muted laoban-contract-hint">
      {{ t('laoban.contract.hint') }}
    </p>
    <div
      v-for="(e, i) in endpoints"
      :key="i"
      class="laoban-contract-row"
    >
      <div class="laoban-contract-head">
        <code class="laoban-contract-method">{{ e.method }}</code>
        <code>{{ e.path }}</code>
        <span
          v-if="e.purpose"
          class="text-muted"
        >{{ e.purpose }}</span>
      </div>
      <div class="laoban-contract-shapes">
        <div>
          <div class="text-muted laoban-contract-label">
            {{ t('laoban.contract.request') }}
          </div>
          <pre v-if="e.request">{{ pretty(e.request) }}</pre>
          <div
            v-else
            class="text-muted"
          >
            {{ t('laoban.contract.none') }}
          </div>
        </div>
        <div>
          <div class="text-muted laoban-contract-label">
            {{ t('laoban.contract.response') }}
          </div>
          <pre v-if="e.response">{{ pretty(e.response) }}</pre>
          <div
            v-else
            class="text-muted"
          >
            {{ t('laoban.contract.none') }}
          </div>
        </div>
      </div>
    </div>
  </details>
</template>

<style lang="scss" scoped>
.laoban-contract {
  margin: 8px 0;
  font-size: 13px;

  summary {
    cursor: pointer;
    font-weight: 600;
  }
}

.laoban-contract-hint {
  margin: 4px 0 8px;
}

.laoban-contract-row {
  padding: 8px 0;
  border-top: 1px solid var(--border);
}

.laoban-contract-head {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 8px;
  align-items: baseline;
  overflow-wrap: anywhere;
}

.laoban-contract-method {
  font-weight: 700;
}

.laoban-contract-shapes {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
  gap: 8px;
  margin-top: 4px;

  pre {
    margin: 2px 0 0;
    padding: 6px 8px;
    max-height: 200px;
    overflow: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: 12px;
  }
}

.laoban-contract-label {
  font-size: 12px;
}
</style>
