<script lang="ts">
// The apps laoban has made (GET /api/apps), newest change first, with each
// app's Fleet state, a link to open it and a link to its detail page (where
// it is changed).
//
// Summaries come from AI output: rendered with {{ }} only, never v-html.
import {
  defineComponent, ref, computed, onMounted, onBeforeUnmount
} from 'vue';
import { useStore } from 'vuex';
import { errorText as errorFor } from '../utils/errors';
import { useText } from '../composables/useText';
import {
  listApps, listBundles, resolveAppUrl, AppSummary, WaitingRound
} from '../utils/backend';
import { refreshWaiting } from '../utils/waiting';
import { bundleReadiness, Bundle, ReadinessState } from '../utils/bundle';
import { appDetailRoute } from '../config/laoban';
import { formatDateTime } from '../utils/time';

export default defineComponent({
  name: 'LaobanAppList',

  setup(_props, { expose }) {
    const store = useStore();
    const { t } = useText(store);

    const apps = ref<AppSummary[]>([]);
    const states = ref<Record<string, ReadinessState>>({});
    const loading = ref(false);
    const loaded = ref(false);
    const errorText = ref('');
    // Rounds that wait for the caller (v0.1.16): marked per app and counted
    // above the list. Read again every WAITING_POLL_MS.
    const waiting = ref<Record<string, WaitingRound>>({});
    const waitingCount = computed(() => Object.values(waiting.value).filter((w) => w.reason === 'confirm').length);
    let waitingTimer: ReturnType<typeof setInterval> | null = null;

    async function loadWaiting() {
      // A nice-to-have, like the Fleet state; also updates the nav count.
      const next: Record<string, WaitingRound> = {};

      for (const w of await refreshWaiting(store)) {
        next[w.app] = w;
      }
      waiting.value = next;
    }

    async function refresh() {
      loading.value = true;
      loadWaiting();
      try {
        const res = await listApps(store);

        apps.value = Array.isArray(res.apps) ? res.apps : [];
        errorText.value = '';
        loaded.value = true;
      } catch (e) {
        errorText.value = errorFor(t, e, 'laoban.apps.loadFailed');
      } finally {
        loading.value = false;
      }

      // Fleet state is a nice-to-have: the list stays usable without it.
      let bundles: Bundle[] = [];

      try {
        bundles = await listBundles(store);
      } catch {
        states.value = {};

        return;
      }
      const next: Record<string, ReadinessState> = {};

      for (const app of apps.value) {
        next[app.appName] = bundleReadiness(bundles, app.fleet?.bundlePrefix || '').state;
      }
      states.value = next;
    }

    function openUrl(app: AppSummary): string | null {
      return resolveAppUrl(app.appUrl, window.location.origin);
    }

    function formatTime(at: string): string {
      if (!at) {
        return '-';
      }
      const d = new Date(at);

      return Number.isNaN(d.getTime()) ? at : formatDateTime(store, d);
    }

    function stateLabel(name: string): string {
      return t(`laoban.apps.state.${ states.value[name] || 'unknown' }`);
    }

    function stateClass(name: string): string {
      switch (states.value[name]) {
      case 'ready':
        return 'text-success';
      case 'error':
        return 'text-error';
      case 'modified':
        return 'laoban-state-modified';
      default:
        return 'text-muted';
      }
    }

    const WAITING_POLL_MS = 30 * 1000;

    onMounted(() => {
      refresh();
      waitingTimer = setInterval(() => {
        if (document.visibilityState !== 'hidden') {
          loadWaiting();
        }
      }, WAITING_POLL_MS);
    });
    onBeforeUnmount(() => {
      if (waitingTimer) {
        clearInterval(waitingTimer);
      }
    });
    expose({ refresh });

    return {
      t, apps, loading, loaded, errorText, waiting, waitingCount, refresh, openUrl, formatTime, stateLabel, stateClass, appDetailRoute
    };
  },
});
</script>

<template>
  <section
    class="laoban-apps"
    :aria-label="t('laoban.apps.title')"
    data-testid="laoban-apps"
  >
    <div class="laoban-apps-head">
      <slot name="actions" />
      <button
        type="button"
        class="btn btn-sm role-secondary"
        :disabled="loading"
        @click="refresh"
      >
        <i
          v-if="loading"
          class="icon icon-spinner icon-spin"
          aria-hidden="true"
        />
        {{ t('laoban.apps.refresh') }}
      </button>
    </div>

    <p
      v-if="waitingCount"
      class="laoban-apps-waiting banner warning"
      role="status"
      data-testid="laoban-apps-waiting"
    >
      {{ t('laoban.waiting.count', { count: waitingCount }) }}
    </p>
    <p
      v-if="errorText"
      class="text-error"
      role="alert"
    >
      {{ errorText }}
    </p>
    <p
      v-else-if="loaded && !apps.length"
      class="text-muted"
    >
      {{ t('laoban.apps.empty') }}
    </p>

    <div
      v-if="apps.length"
      class="laoban-apps-scroll"
    >
      <table class="laoban-apps-table">
        <thead>
          <tr>
            <th>{{ t('laoban.apps.col.name') }}</th>
            <th>{{ t('laoban.apps.col.summary') }}</th>
            <th>{{ t('laoban.apps.col.updated') }}</th>
            <th>{{ t('laoban.apps.col.state') }}</th>
            <th><span class="sr-only">{{ t('laoban.apps.col.actions') }}</span></th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="app in apps"
            :key="app.appName"
          >
            <td class="laoban-apps-name">
              <router-link :to="appDetailRoute(app.appName)">
                {{ app.appName }}
              </router-link>
            </td>
            <td class="laoban-text">
              <span v-if="app.editable">{{ app.summary }}</span>
              <span
                v-if="app.round"
                class="laoban-apps-round"
              >{{ t(`laoban.round.states.${ app.round }`) }}</span>
              <span
                v-if="waiting[app.appName]"
                class="laoban-apps-waiting-badge"
                :class="{ mine: waiting[app.appName].reason === 'confirm' }"
              >{{ t(`laoban.waiting.badge.${ waiting[app.appName].reason }`) }}</span>
              <span
                v-if="!app.editable"
                class="text-muted"
              >{{ t('laoban.apps.legacy') }}</span>
            </td>
            <td class="laoban-apps-nowrap">
              {{ formatTime(app.updatedAt) }}
              <span
                v-if="app.revisions > 1"
                class="text-muted"
              >{{ t('laoban.apps.revisions', { count: app.revisions }) }}</span>
            </td>
            <td :class="['laoban-apps-nowrap', stateClass(app.appName)]">
              {{ stateLabel(app.appName) }}
            </td>
            <td class="laoban-apps-actions">
              <a
                v-if="openUrl(app)"
                :href="openUrl(app)"
                target="_blank"
                rel="noopener noreferrer"
                class="btn btn-sm role-secondary"
              >{{ t('laoban.apps.open') }}</a>
              <router-link
                :to="appDetailRoute(app.appName)"
                class="btn btn-sm role-primary"
              >
                {{ app.editable ? t('laoban.apps.edit') : t('laoban.apps.details') }}
              </router-link>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>
</template>

<style lang="scss" scoped>
.laoban-apps-waiting-badge {
  display: inline-block;
  margin-left: 6px;
  padding: 0 6px;
  border-radius: var(--border-radius);
  font-size: 11px;
  border: 1px solid var(--border);

  &.mine {
    border-color: var(--warning);
    background: var(--warning-banner-bg, transparent);
    font-weight: 600;
  }
}

.laoban-apps-waiting {
  margin: 0 0 8px;
}

.laoban-apps-round {
  display: inline-block;
  margin-left: 6px;
  padding: 0 6px;
  border: 1px solid var(--primary);
  border-radius: var(--border-radius);
  font-size: 11px;
  color: var(--link);
}

// Rancher's text-warning yellow is too light on white; see palettes.scss.
.laoban-state-modified {
  color: var(--laoban-text-warning);
}

.laoban-apps-head {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  margin-bottom: 8px;
}

.laoban-apps-scroll {
  overflow-x: auto;
}

.laoban-apps-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;

  th,
  td {
    text-align: left;
    vertical-align: top;
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
  }

  th {
    color: var(--muted);
    font-weight: normal;
  }
}

.laoban-apps-name {
  font-family: monospace;
  white-space: nowrap;
}

.laoban-apps-nowrap {
  white-space: nowrap;

  span {
    display: block;
  }
}

.laoban-apps-actions {
  white-space: nowrap;
  text-align: right;

  .btn + .btn {
    margin-left: 6px;
  }
}

.laoban-text {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
}
</style>
