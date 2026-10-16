<script lang="ts">
// Past rounds of 意見收斂 on an app, read from the archive (the
// laoban-rounds branch). Loaded when opened; one round at a time is read.
// The archive is masked, as everything laoban writes to GitHub.
//
// Opinions and plans are user and AI text: rendered with {{ }} only.
import {
  defineComponent, ref, PropType
} from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import {
  listPastRounds, getPastRound, PastRoundEntry, PastRound, RequestError
} from '../utils/backend';
import { formatDateTime } from '../utils/time';
import { userLabel } from '../utils/users';
import OpinionList from './OpinionList.vue';

export default defineComponent({
  name: 'LaobanPastRounds',

  components: { OpinionList },

  props: {
    app: {
      type:     String,
      required: true,
    },
    // Message for an error the page does not know (from useDeployFlow).
    messageFor: {
      type:    Function as PropType<(e: unknown) => string>,
      default: null,
    },
  },

  setup(props) {
    const store = useStore();
    const { t } = useText(store);
    const loaded = ref(false);
    const loading = ref(false);
    const error = ref('');
    const entries = ref<PastRoundEntry[]>([]);
    const truncated = ref(false);
    const shown = ref<string | null>(null);
    const detail = ref<PastRound | null>(null);
    const detailLoading = ref(false);

    function fail(e: unknown): string {
      if (e instanceof RequestError && e.userMessage) {
        return e.userMessage;
      }

      return props.messageFor ? props.messageFor(e) : t('laoban.pastRounds.loadFailed');
    }

    async function load() {
      if (loaded.value || loading.value) {
        return;
      }
      loading.value = true;
      error.value = '';
      try {
        const res = await listPastRounds(store, props.app);

        entries.value = res.rounds || [];
        truncated.value = !!res.truncated;
        loaded.value = true;
      } catch (e) {
        error.value = fail(e);
      } finally {
        loading.value = false;
      }
    }

    async function show(id: string) {
      if (shown.value === id) {
        shown.value = null;
        detail.value = null;

        return;
      }
      shown.value = id;
      detail.value = null;
      detailLoading.value = true;
      error.value = '';
      try {
        const got = await getPastRound(store, props.app, id);

        // Another round was opened meanwhile: this answer is not for it.
        if (shown.value === id) {
          detail.value = got;
        }
      } catch (e) {
        if (shown.value === id) {
          error.value = fail(e);
          shown.value = null;
        }
      } finally {
        if (shown.value === id || shown.value === null) {
          detailLoading.value = false;
        }
      }
    }

    function onToggle(ev: Event) {
      if ((ev.target as HTMLDetailsElement).open) {
        load();
      }
    }

    const who = (id: string) => (id === 'laoban' ? 'laoban' : userLabel(store, id));
    // Log lines in the page's language when the code is known (as the
    // round panel shows them); the archived Chinese text otherwise.
    const KNOWN = ['opened', 'opinion', 'objected', 'converging', 'converged', 'failed', 'confirmed', 'skipped', 'applying', 'done', 'cancelled',
      'supported', 'unsupported', 'replied', 'restarted', 'edited', 'withdrawn', 'deadline', 'commented', 'emptyCancelled'];

    function logText(l: { event: string; code?: string; args?: Record<string, string> }): string {
      if (!l.code || !KNOWN.includes(l.code) || (l.code === 'failed' && !l.args?.code)) {
        return l.event;
      }
      const args = { ...(l.args || {}) };

      if (args.opinion) {
        args.opinion = `#${ String(args.opinion).replace(/^o/, '') }`;
      }
      if (args.user) {
        args.user = who(args.user);
      }
      if (l.code === 'failed') {
        args.message = t(`laoban.round.failures.${ args.code }`, args);
      }

      const code = l.code === 'deadline' && args.minutes === '0' ? 'deadlineNow' : l.code;

      return t(`laoban.round.events.${ code }`, args);
    }
    const formatTime = (at?: string) => {
      if (!at) {
        return '';
      }
      const d = new Date(at);

      return Number.isNaN(d.getTime()) ? at : formatDateTime(store, d);
    };

    return {
      t, loaded, loading, error, entries, truncated, shown, detail, detailLoading, show, onToggle, who, logText, formatTime
    };
  },
});
</script>

<template>
  <details
    class="laoban-detail-section laoban-past-rounds"
    data-testid="laoban-past-rounds"
    @toggle="onToggle"
  >
    <summary>{{ t('laoban.pastRounds.title') }}<template v-if="loaded">（{{ entries.length }}{{ truncated ? '+' : '' }}）</template></summary>
    <p
      v-if="loading"
      class="text-muted"
    >
      <i
        class="icon icon-spinner icon-spin"
        aria-hidden="true"
      />
      {{ t('laoban.pastRounds.loading') }}
    </p>
    <p
      v-if="error"
      class="text-error"
      role="alert"
    >
      {{ error }}
    </p>
    <p
      v-if="loaded && !entries.length"
      class="text-muted laoban-hint"
    >
      {{ t('laoban.pastRounds.none') }}
    </p>
    <ul class="laoban-past-list">
      <li
        v-for="e in entries"
        :key="e.id"
      >
        <button
          type="button"
          class="btn btn-sm role-link"
          :aria-expanded="shown === e.id ? 'true' : 'false'"
          @click="show(e.id)"
        >
          {{ e.date }} · {{ e.id }}
        </button>
        <div
          v-if="shown === e.id"
          class="laoban-past-round"
        >
          <p
            v-if="detailLoading"
            class="text-muted"
          >
            {{ t('laoban.pastRounds.loading') }}
          </p>
          <template v-else-if="detail">
            <p>
              <strong>{{ t(`laoban.round.states.${ detail.round.state }`) }}</strong>
              <span class="text-muted">
                · {{ t('laoban.round.startedBy', { by: who(detail.round.initiator) }) }}
                <template v-if="detail.openedAt">· {{ formatTime(detail.openedAt) }}</template>
              </span>
            </p>
            <OpinionList
              :opinions="detail.round.opinions"
              :can-discuss="false"
            />
            <div
              v-if="detail.round.plan"
              class="laoban-past-plan"
            >
              <h4>{{ t('laoban.round.planTitle') }}</h4>
              <p class="laoban-text">
                {{ detail.round.plan.summary }}
              </p>
              <ol>
                <li
                  v-for="(s, i) in detail.round.plan.steps"
                  :key="i"
                  class="laoban-text"
                >
                  {{ s }}
                </li>
              </ol>
            </div>
            <p
              v-if="detail.round.result && detail.round.result.commit"
              class="text-muted"
            >
              {{ t('laoban.pastRounds.commit', { commit: String(detail.round.result.commit).slice(0, 7) }) }}
            </p>
            <details class="laoban-round-log">
              <summary>{{ t('laoban.round.log', { count: detail.round.log.length }) }}</summary>
              <ul>
                <li
                  v-for="(l, i) in detail.round.log"
                  :key="i"
                >
                  <span class="text-muted">{{ formatTime(l.at) }}</span>
                  {{ t('laoban.round.logLine', { who: who(l.by), event: logText(l) }) }}
                </li>
              </ul>
            </details>
          </template>
        </div>
      </li>
    </ul>
  </details>
</template>

<style lang="scss" scoped>
.laoban-past-list {
  list-style: none;
  padding: 0;
  margin: 4px 0;
}

.laoban-past-round {
  margin: 4px 0 12px 8px;
  padding-left: 8px;
  border-left: 2px solid var(--border);

  h4 {
    margin: 8px 0 4px;
    font-size: 13px;
  }
}

.laoban-text {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
</style>
