<script lang="ts">
// The apps laoban has made (GET /api/apps), newest change first, with each
// app's Fleet state, a link to open it and an "edit" action that hands the
// app name to the page (emit 'edit').
//
// Summaries come from AI output: rendered with {{ }} only, never v-html.
import {
  defineComponent, ref, onMounted, PropType
} from 'vue';
import { useStore } from 'vuex';
import { useI18n } from '@shell/composables/useI18n';
import {
  listApps, listBundles, resolveAppUrl, RequestError, AppSummary
} from '../utils/backend';
import { bundleReadiness, Bundle, ReadinessState } from '../utils/bundle';

export default defineComponent({
  name: 'LaobanAppList',

  props: {
    // App currently loaded into the editor above, highlighted in the list.
    editingApp: {
      type:    String as PropType<string | null>,
      default: null,
    },
    // Disable "edit" while a plan or apply is running.
    busy: {
      type:    Boolean,
      default: false,
    },
  },

  emits: ['edit'],

  setup(_props, { expose }) {
    const store = useStore();
    const { t } = useI18n(store);

    const apps = ref<AppSummary[]>([]);
    const states = ref<Record<string, ReadinessState>>({});
    const loading = ref(false);
    const loaded = ref(false);
    const errorText = ref('');

    async function refresh() {
      loading.value = true;
      try {
        const res = await listApps(store);

        apps.value = Array.isArray(res.apps) ? res.apps : [];
        errorText.value = '';
        loaded.value = true;
      } catch (e) {
        errorText.value = e instanceof RequestError && e.userMessage ? e.userMessage : t('laoban.apps.loadFailed');
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

      return Number.isNaN(d.getTime()) ? at : d.toLocaleString();
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
        return 'text-warning';
      default:
        return 'text-muted';
      }
    }

    onMounted(refresh);
    expose({ refresh });

    return {
      t, apps, loading, loaded, errorText, refresh, openUrl, formatTime, stateLabel, stateClass
    };
  },
});
</script>

<template>
  <section
    class="laoban-panel laoban-apps"
    :aria-label="t('laoban.apps.title')"
    data-testid="laoban-apps"
  >
    <div class="laoban-apps-head">
      <h2>{{ t('laoban.apps.title') }}</h2>
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
            :class="{ 'laoban-apps-current': app.appName === editingApp }"
          >
            <td class="laoban-apps-name">
              {{ app.appName }}
            </td>
            <td class="laoban-text">
              <span v-if="app.editable">{{ app.summary }}</span>
              <span
                v-else
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
              <button
                type="button"
                class="btn btn-sm role-primary"
                :disabled="!app.editable || busy"
                :title="app.editable ? '' : t('laoban.apps.legacy')"
                @click="$emit('edit', app.appName)"
              >
                {{ t('laoban.apps.edit') }}
              </button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>
</template>

<style lang="scss" scoped>
.laoban-apps-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 8px;

  h2 {
    margin: 0;
  }
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

.laoban-apps-current {
  background: var(--sortable-table-selected-bg, var(--accent-btn));
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
