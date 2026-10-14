<script lang="ts">
// One app (GET /api/apps/<name>): what it does, its versions and its 3D
// architecture, plus "要怎麼修改" which runs the shared plan/deploy flow with
// baseApp. A version can be viewed (optionally compared with the current
// one in the 3D view) and restored, which publishes it again as a new
// version. A change plan is always shown compared with the current version.
// After a change or restore runs, the app and its versions are reloaded.
//
// Text from the backend or AI is rendered with {{ }} only, never v-html.
import {
  defineComponent, ref, computed, onMounted, watch
} from 'vue';
import { useStore } from 'vuex';
import { useRoute } from 'vue-router';
import { useI18n } from '@shell/composables/useI18n';
import AppearanceMenu from '../components/AppearanceMenu.vue';
import { useAppearance } from '../composables/useAppearance';
import ArchitectureView from '../components/ArchitectureView.vue';
import PlanResult from '../components/PlanResult.vue';
import StatusBar from '../components/StatusBar.vue';
import VersionList from '../components/VersionList.vue';
import { useDeployFlow } from '../composables/useDeployFlow';
import {
  getApp, listVersions, getVersion, resolveAppUrl, safeHttpsUrl, AppDetail, VersionEntry, VersionDetail
} from '../utils/backend';
import { mergeForDiff } from '../utils/graphDiff';
import { APPS_ROUTE_NAME, PRODUCT_NAME, BLANK_CLUSTER } from '../config/laoban';

export default defineComponent({
  name: 'LaobanAppDetail',

  components: {
    ArchitectureView, PlanResult, StatusBar, AppearanceMenu, VersionList
  },

  setup() {
    const store = useStore();
    const route = useRoute();
    const { t } = useI18n(store);
    const { palette } = useAppearance(store);

    const detail = ref<AppDetail | null>(null);
    const loading = ref(false);
    const loadError = ref('');
    const prompt = ref('');
    const versions = ref<VersionEntry[]>([]);
    const versionsTruncated = ref(false);
    const versionsLoading = ref(false);
    const versionsError = ref('');
    // Commit of the version being viewed; null means the current one.
    const selectedCommit = ref<string | null>(null);
    const selectedVersion = ref<VersionDetail | null>(null);
    const compare = ref(true);

    const name = computed(() => String(route.params.name || ''));

    async function load() {
      if (!name.value) {
        return;
      }
      loading.value = true;
      try {
        detail.value = await getApp(store, name.value);
        loadError.value = '';
      } catch (e) {
        detail.value = null;
        loadError.value = flow.messageFor(e);
      } finally {
        loading.value = false;
      }
    }

    async function loadVersions() {
      if (!name.value) {
        return;
      }
      versionsLoading.value = true;
      try {
        const res = await listVersions(store, name.value);

        versions.value = Array.isArray(res.versions) ? res.versions : [];
        versionsTruncated.value = !!res.truncated;
        versionsError.value = '';
      } catch (e) {
        versions.value = [];
        versionsError.value = flow.messageFor(e);
      } finally {
        versionsLoading.value = false;
      }
    }

    async function selectVersion(commit: string | null) {
      selectedCommit.value = commit;
      selectedVersion.value = null;
      if (!commit) {
        return;
      }
      try {
        const v = await getVersion(store, name.value, commit);

        if (selectedCommit.value === commit) {
          selectedVersion.value = v;
        }
      } catch (e) {
        versionsError.value = flow.messageFor(e);
        selectedCommit.value = null;
      }
    }

    function reloadAll() {
      selectVersion(null);
      load();
      loadVersions();
    }

    const flow = useDeployFlow(store, t, { onReady: reloadAll });

    const busy = computed(() => flow.busy.value || loading.value);
    const canPlan = computed(() => !busy.value && !!detail.value && !!prompt.value.trim());
    const appsRoute = { name: APPS_ROUTE_NAME, params: { product: PRODUCT_NAME, cluster: BLANK_CLUSTER } };
    const openLink = computed(() => resolveAppUrl(detail.value?.appUrl, window.location.origin));
    const sourceLink = computed(() => {
      const repo = flow.config.value?.repo;
      const path = detail.value?.path;

      return repo && path ? safeHttpsUrl(`https://github.com/${ repo }/tree/main/${ path }`) : null;
    });
    const selectedNumber = computed(() => versions.value.find((v) => v.commit === selectedCommit.value)?.version ?? 0);

    // What the 3D view shows:
    // - a change plan: the change compared with the current version;
    // - a selected earlier version: that version, compared with the current
    //   one when "compare" is on (marks = what restoring it would change);
    // - otherwise the current version.
    const view = computed(() => {
      const current = detail.value?.architecture || null;

      if (flow.plan.value) {
        const d = mergeForDiff(current, flow.plan.value.architecture);

        return { architecture: d.architecture, diff: d, hint: t('laoban.versions.hintPlan') };
      }
      if (selectedVersion.value) {
        if (compare.value) {
          const d = mergeForDiff(current, selectedVersion.value.architecture);

          return { architecture: d.architecture, diff: d, hint: t('laoban.versions.hintCompare', { n: selectedNumber.value }) };
        }

        return { architecture: selectedVersion.value.architecture, diff: null, hint: t('laoban.versions.hintVersion', { n: selectedNumber.value }) };
      }

      return { architecture: current, diff: null, hint: t('laoban.edit.currentArch') };
    });

    function restore(commit: string) {
      if (!busy.value && detail.value) {
        flow.restore(detail.value.appName, commit);
      }
    }
    const currentSteps = computed(() => (Array.isArray(detail.value?.steps) ? detail.value!.steps.map(String) : []));

    function generatePlan() {
      if (canPlan.value && detail.value) {
        flow.generatePlan(prompt.value, { baseApp: detail.value.appName });
      }
    }

    function formatTime(at: string): string {
      const d = new Date(at);

      return Number.isNaN(d.getTime()) ? at : d.toLocaleString();
    }

    watch(name, () => {
      flow.reset();
      prompt.value = '';
      reloadAll();
    });

    onMounted(() => {
      flow.loadConfig();
      load();
      loadVersions();
    });

    return {
      t,
      palette,
      ...flow,
      busy,
      detail,
      loading,
      loadError,
      prompt,
      name,
      canPlan,
      appsRoute,
      openLink,
      sourceLink,
      view,
      versions,
      versionsTruncated,
      versionsLoading,
      versionsError,
      selectedCommit,
      selectedVersion,
      selectedNumber,
      compare,
      selectVersion,
      restore,
      currentSteps,
      generatePlan,
      formatTime,
    };
  },
});
</script>

<template>
  <div
    class="laoban-m0"
    :data-laoban-palette="palette"
  >
    <AppearanceMenu class="laoban-appearance-bar" />
    <header class="laoban-header">
      <div>
        <router-link :to="appsRoute">
          {{ t('laoban.detail.back') }}
        </router-link>
        <h1 class="laoban-detail-name">
          {{ name }}
        </h1>
        <p
          v-if="detail"
          class="text-muted laoban-text"
        >
          {{ detail.summary }}
        </p>
      </div>
      <div
        v-if="detail"
        class="laoban-detail-actions"
      >
        <a
          v-if="sourceLink"
          :href="sourceLink"
          target="_blank"
          rel="noopener noreferrer"
          class="btn role-secondary"
        >{{ t('laoban.detail.source') }}</a>
        <a
          v-if="openLink"
          :href="openLink"
          target="_blank"
          rel="noopener noreferrer"
          class="btn role-primary"
        >{{ t('laoban.dashboard.result.openApp') }}</a>
      </div>
    </header>

    <p
      v-if="loading && !detail"
      class="text-muted"
    >
      <i
        class="icon icon-spinner icon-spin"
        aria-hidden="true"
      />
      {{ t('laoban.detail.loading') }}
    </p>
    <p
      v-else-if="loadError"
      class="text-error"
      role="alert"
    >
      {{ loadError }}
    </p>

    <div
      v-if="detail"
      class="laoban-work"
    >
      <section
        class="laoban-panel"
        :aria-label="t('laoban.edit.promptLabel')"
      >
        <h2>{{ t('laoban.detail.changeTitle') }}</h2>
        <form
          class="laoban-form"
          @submit.prevent="generatePlan"
        >
          <label
            for="laoban-change"
            class="laoban-label"
          >{{ t('laoban.edit.promptLabel') }}</label>
          <textarea
            id="laoban-change"
            v-model="prompt"
            rows="4"
            maxlength="4000"
            :placeholder="t('laoban.edit.promptPlaceholder')"
            :disabled="busy"
          />
          <button
            type="submit"
            class="btn role-primary"
            :disabled="!canPlan"
          >
            <i
              v-if="phase === 'planning'"
              class="icon icon-spinner icon-spin"
              aria-hidden="true"
            />
            {{ t('laoban.edit.generate') }}
          </button>
        </form>

        <p
          v-if="errorText"
          class="laoban-error text-error"
          role="alert"
        >
          {{ errorText }}
        </p>

        <PlanResult
          :plan="plan"
          :steps="steps"
          :applied="applied"
          :phase="phase"
          :can-apply="canApply"
          :commit-link="commitLink"
          :short-commit="shortCommit"
          :app-link="appLink"
          @apply="confirmAndDeploy"
        />

        <details
          class="laoban-detail-section"
          :open="!plan"
        >
          <summary>{{ t('laoban.detail.currentSteps') }}</summary>
          <ol>
            <li
              v-for="(step, i) in currentSteps"
              :key="i"
              class="laoban-text"
            >
              {{ step }}
            </li>
          </ol>
        </details>

        <div class="laoban-detail-section">
          <h3>{{ t('laoban.versions.title', { count: versions.length }) }}</h3>
          <VersionList
            :versions="versions"
            :selected="selectedCommit"
            :busy="busy"
            :loading="versionsLoading"
            :error="versionsError"
            :truncated="versionsTruncated"
            @select="selectVersion"
            @restore="restore"
          />
        </div>
      </section>

      <section
        class="laoban-panel laoban-view"
        :aria-label="t('laoban.dashboard.arch.title')"
      >
        <h2>{{ t('laoban.dashboard.arch.title') }}</h2>
        <div class="laoban-view-head">
          <p class="text-muted laoban-hint">
            {{ view.hint }}
          </p>
          <label
            v-if="selectedVersion && !plan"
            class="laoban-compare"
          >
            <input
              v-model="compare"
              type="checkbox"
            >
            {{ t('laoban.versions.compare') }}
          </label>
          <button
            v-if="selectedVersion && !plan"
            type="button"
            class="btn btn-sm role-secondary"
            @click="selectVersion(null)"
          >
            {{ t('laoban.versions.backToCurrent') }}
          </button>
        </div>
        <p
          v-if="selectedVersion && !plan"
          class="laoban-text laoban-hint"
        >
          {{ selectedVersion.summary }}
        </p>
        <ArchitectureView
          v-if="view.architecture"
          :architecture="view.architecture"
          :diff="view.diff"
        />
      </section>
    </div>

    <StatusBar
      v-if="detail"
      :text="statusText"
      :spinning="busy || phase === 'deploying'"
      :message="showStatusMessage && readiness ? readiness.message : ''"
      :poll-error="phase === 'deploying' && pollError"
    />
  </div>
</template>

<style lang="scss" scoped>
@import '../styles/layout.scss';

.laoban-appearance-bar {
  justify-content: flex-end;
}

.laoban-detail-name {
  font-family: monospace;
  margin-top: 4px;
}

.laoban-detail-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.laoban-detail-section {
  margin-top: 16px;
  padding-top: 12px;
  border-top: 1px solid var(--border);

  summary {
    cursor: pointer;
    font-weight: bold;
  }

  ol {
    padding-left: 20px;
    margin: 8px 0 0;
  }

  h3 {
    margin: 0 0 8px;
  }
}

.laoban-view-head {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 12px;
  align-items: center;
  margin-bottom: 6px;
}

.laoban-compare {
  display: inline-flex;
  gap: 4px;
  align-items: center;
  font-size: 12px;
}

.laoban-history {
  list-style: none;
  padding: 0;
  margin: 0;
  font-size: 12px;

  li {
    display: flex;
    flex-direction: column;
    margin-bottom: 6px;
  }
}
</style>
