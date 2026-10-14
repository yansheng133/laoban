<script lang="ts">
// One app (GET .../apps/<name>): what it does, its versions and its 3D
// architecture. Changes go through 意見收斂 (components/RoundPanel.vue,
// composables/useRound.ts): opinions from everyone, converged by the AI
// into one plan, confirmed by those who gave them, then published. The
// converged plan is shown in the 3D view compared with the current version.
//
// Without AI: the code can be edited by hand (程式碼 tab; files in a running
// round's scope wait for it), the latest version's 3D layout rearranged
// (儲存版面), and an earlier version restored (not while a round runs).
// Structure changes in the 3D view become sentences in the opinion.
// After any change is deployed, the app and its versions are reloaded.
//
// Text from the backend or AI is rendered with {{ }} only, never v-html.
import {
  defineComponent, ref, computed, onMounted, onBeforeUnmount, watch
} from 'vue';
import { useStore } from 'vuex';
import { useRoute } from 'vue-router';
import { useText } from '../composables/useText';
import AppearanceMenu from '../components/AppearanceMenu.vue';
import { useAppearance } from '../composables/useAppearance';
import ArchitectureView from '../components/ArchitectureView.vue';
import RoundPanel from '../components/RoundPanel.vue';
import StatusBar from '../components/StatusBar.vue';
import VersionList from '../components/VersionList.vue';
import CodeEditor from '../components/CodeEditor.vue';
import StructureEdit from '../components/StructureEdit.vue';
import { useDeployFlow } from '../composables/useDeployFlow';
import { useRound } from '../composables/useRound';
import {
  getApp, listVersions, getVersion, resolveAppUrl, safeHttpsUrl, saveFiles, saveLayout, aiRequestTimeoutMs,
  AppDetail, VersionEntry, VersionDetail, AppFiles, Position, RoundAction
} from '../utils/backend';
import { appendLine } from '../utils/structure';
import { mergeForDiff } from '../utils/graphDiff';
import { APPS_ROUTE_NAME, PRODUCT_NAME, BLANK_CLUSTER } from '../config/laoban';
import { formatDateTime } from '../utils/time';

export default defineComponent({
  name: 'LaobanAppDetail',

  components: {
    ArchitectureView, RoundPanel, StatusBar, AppearanceMenu, VersionList, CodeEditor, StructureEdit
  },

  setup() {
    const store = useStore();
    const route = useRoute();
    const { t } = useText(store);
    const { palette } = useAppearance(store);

    const detail = ref<AppDetail | null>(null);
    const loading = ref(false);
    const loadError = ref('');
    // The opinion being typed (structure changes add sentences to it).
    const opinionDraft = ref('');
    const versions = ref<VersionEntry[]>([]);
    const versionsTruncated = ref(false);
    const versionsLoading = ref(false);
    const versionsError = ref('');
    // Commit of the version being viewed; null means the current one.
    const selectedCommit = ref<string | null>(null);
    const selectedVersion = ref<VersionDetail | null>(null);
    const compare = ref(true);
    const tab = ref<'arch' | 'code'>('arch');
    // Dragged but not yet saved positions, by node id.
    const pending = ref<Record<string, Position>>({});
    const selectedNode = ref<string | null>(null);
    const editorReload = ref(0);
    const editorDirty = ref(false);
    // While a round waits for confirmation the 3D view shows its plan; this
    // switches back to the current version (to rearrange it, for example).
    const showPlan = ref(true);
    // Someone else published a version this page has not loaded.
    const staleNotice = ref('');

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
      staleNotice.value = '';
      selectVersion(null);
      pending.value = {};
      editorReload.value++;
      load();
      loadVersions();
    }

    const flow = useDeployFlow(store, t, { onReady: reloadAll });
    const round = useRound(store, {
      onApplied:    (applied) => flow.watchApplied(applied),
      messageFor:   flow.messageFor,
      aiTimeoutMs:  () => aiRequestTimeoutMs(flow.config.value),
      lostMessage:  () => t('laoban.round.lost'),
    });

    // Permissions are checked by the Kubernetes API server; the page lets
    // everyone try and shows its answer.
    const canEdit = computed(() => !!detail.value);
    const busy = computed(() => flow.busy.value || loading.value || !!round.pending.value);
    const deploying = computed(() => flow.phase.value === 'deploying');
    const currentCommit = computed(() => versions.value.find((v) => v.current)?.commit || '');
    const roundPlan = computed(() => (round.round.value?.state === 'confirming' ? round.round.value.plan : null));
    const showingPlan = computed(() => !!roundPlan.value && showPlan.value);

    watch(() => roundPlan.value?.planId, () => {
      showPlan.value = true;
    });
    // Files a running round is about: the code editor leaves them alone.
    const blockedFiles = computed(() => {
      const r = round.round.value;

      if (!r || !round.active.value) {
        return [];
      }

      return r.scope.flatMap((p) => (p === 'web' ? ['web/index.html', 'web/app.js', 'web/style.css'] : ['api/handler.py']));
    });
    // Only the latest version, as it is, can be rearranged.
    const canArrange = computed(() => canEdit.value && !busy.value && !showingPlan.value && !selectedVersion.value && !selectedCommit.value);
    const pendingCount = computed(() => Object.keys(pending.value).length);
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

      if (showingPlan.value && roundPlan.value && !selectedVersion.value) {
        const d = mergeForDiff(current, roundPlan.value.architecture);
        const same = !d.summary.added && !d.summary.removed && !d.summary.changed;

        return { architecture: d.architecture, diff: d, hint: t(same ? 'laoban.round.hintPlanSame' : 'laoban.round.hintPlan') };
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
      if (!busy.value && canEdit.value && detail.value && !round.active.value) {
        flow.restore(detail.value.appName, commit);
      }
    }

    function onMove(id: string, pos: Position) {
      if (canArrange.value) {
        pending.value = { ...pending.value, [id]: pos };
      }
    }

    function discardLayout() {
      pending.value = {};
      if (staleNotice.value && !editorDirty.value) {
        reloadAll();
      }
    }

    function saveLayoutNow() {
      const app = detail.value?.appName;

      if (!app || !pendingCount.value || !canArrange.value || !currentCommit.value) {
        return;
      }
      const positions = { ...pending.value };

      flow.publishDirect('layout', (timeoutMs) => saveLayout(store, app, currentCommit.value, positions, timeoutMs));
    }

    function saveCode(payload: { baseCommit: string; files: AppFiles; note: string }) {
      const app = detail.value?.appName;

      if (!app || !canEdit.value || busy.value) {
        return;
      }
      flow.publishDirect('manual', (timeoutMs) => saveFiles(store, app, payload.baseCommit, payload.files, payload.note || t('laoban.editor.defaultNote'), timeoutMs));
    }

    function clearError() {
      if (!busy.value) {
        flow.errorText.value = '';
      }
      // Changes discarded while a newer version waits: load it now.
      if (staleNotice.value && !pendingCount.value) {
        reloadAll();
      }
    }

    function addStructure(line: string) {
      opinionDraft.value = appendLine(opinionDraft.value, line, 1000);
    }

    async function openRound(opts: { minutes: number; scope: string }) {
      if (await round.open(opinionDraft.value.trim(), opts.minutes, opts.scope)) {
        opinionDraft.value = '';
      }
    }

    async function roundAct(action: RoundAction, body: Record<string, string>) {
      if (await round.act(action, body) && action === 'opinions') {
        opinionDraft.value = '';
      }
    }

    // The deploy's progress, shown under the round as well.
    const deployNotice = computed(() => (flow.phase.value === 'deploying' ? flow.statusText.value : ''));
    const roundNotice = computed(() => round.notice.value || deployNotice.value || staleNotice.value);
    const detailStatus = computed(() => (flow.phase.value === 'idle' ? t('laoban.detail.idle') : flow.statusText.value));

    // Every 15 s: has someone else published a version? Reload when nothing
    // unsaved would be lost; otherwise say so.
    const VERSION_POLL_MS = 15 * 1000;
    let versionTimer: ReturnType<typeof setInterval> | null = null;

    async function checkVersions() {
      const app = name.value;

      if (!app || busy.value || document.visibilityState === 'hidden' || !currentCommit.value) {
        return;
      }
      try {
        const res = await listVersions(store, app);
        const latest = (res.versions || []).find((v) => v.current)?.commit;

        if (app !== name.value || !latest || latest === currentCommit.value || busy.value) {
          return;
        }
        if (editorDirty.value || pendingCount.value) {
          staleNotice.value = t('laoban.detail.stale');
        } else {
          reloadAll();
        }
      } catch {
        // Next time.
      }
    }

    const currentSteps = computed(() => (Array.isArray(detail.value?.steps) ? detail.value!.steps.map(String) : []));

    function formatTime(at: string): string {
      const d = new Date(at);

      return Number.isNaN(d.getTime()) ? at : formatDateTime(store, d);
    }

    watch(name, (n) => {
      flow.reset();
      opinionDraft.value = '';
      selectedNode.value = null;
      tab.value = 'arch';
      reloadAll();
      if (n) {
        round.start(n);
      }
    });

    // Leaving with unsaved code: the browser's own "leave page?" prompt.
    function beforeUnload(ev: BeforeUnloadEvent) {
      if (editorDirty.value || pendingCount.value) {
        ev.preventDefault();
      }
    }

    onMounted(() => {
      flow.loadConfig();
      load();
      loadVersions();
      if (name.value) {
        round.start(name.value);
      }
      window.addEventListener('beforeunload', beforeUnload);
      versionTimer = setInterval(checkVersions, VERSION_POLL_MS);
    });

    onBeforeUnmount(() => {
      window.removeEventListener('beforeunload', beforeUnload);
      if (versionTimer) {
        clearInterval(versionTimer);
      }
    });

    return {
      t,
      palette,
      ...flow,
      busy,
      detail,
      loading,
      loadError,
      opinionDraft,
      name,
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
      formatTime,
      canEdit,
      deploying,
      blockedFiles,
      roundPlan,
      showPlan,
      showingPlan,
      roundNotice,
      detailStatus,
      roundState: round.round,
      roundActive: round.active,
      roundPending: round.pending,
      roundElapsed: round.elapsed,
      roundError: round.error,
      openRound,
      roundAct,
      canArrange,
      tab,
      pending,
      pendingCount,
      selectedNode,
      editorReload,
      editorDirty,
      onMove,
      discardLayout,
      saveLayoutNow,
      saveCode,
      addStructure,
      clearError,
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
        <RoundPanel
          v-model:draft="opinionDraft"
          :round="roundState"
          :pending="roundPending"
          :elapsed="roundElapsed"
          :error="roundError"
          :disabled="deploying || phase === 'applying'"
          :notice="roundNotice"
          @open="openRound"
          @act="roundAct"
        />

        <p
          v-if="errorText"
          class="laoban-error text-error"
          role="alert"
        >
          {{ errorText }}
        </p>

        <details
          class="laoban-detail-section"
          :open="!roundPlan"
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
            :read-only="!canEdit || roundActive"
            @select="selectVersion"
            @restore="restore"
          />
        </div>
      </section>

      <section
        class="laoban-panel laoban-view"
        :aria-label="t('laoban.dashboard.arch.title')"
      >
        <div
          class="laoban-tabs"
          role="tablist"
        >
          <button
            v-for="k in ['arch', 'code']"
            :key="k"
            type="button"
            role="tab"
            class="laoban-tab"
            :class="{ active: tab === k }"
            :aria-selected="tab === k"
            @click="tab = k"
          >
            {{ t(`laoban.detail.tabs.${ k }`) }}<span
              v-if="k === 'code' && editorDirty"
              aria-hidden="true"
            > ●</span>
          </button>
        </div>
        <div
          v-show="tab === 'code'"
          role="tabpanel"
        >
          <p
            v-if="errorText"
            class="laoban-error text-error"
            role="alert"
          >
            {{ errorText }}
          </p>
          <CodeEditor
            :app-name="detail.appName"
            :visible="tab === 'code'"
            :editable="canEdit"
            :busy="busy || phase === 'deploying'"
            :blocked-files="blockedFiles"
            :reload-key="editorReload"
            @save="saveCode"
            @dirty="editorDirty = $event"
            @discard="clearError"
          />
        </div>
        <div
          v-show="tab === 'arch'"
          role="tabpanel"
          class="laoban-arch-panel"
        >
        <div class="laoban-view-head">
          <p class="text-muted laoban-hint">
            {{ view.hint }}
          </p>
          <label
            v-if="selectedVersion"
            class="laoban-compare"
          >
            <input
              v-model="compare"
              type="checkbox"
            >
            {{ t('laoban.versions.compare') }}
          </label>
          <button
            v-if="selectedVersion"
            type="button"
            class="btn btn-sm role-secondary"
            @click="selectVersion(null)"
          >
            {{ t('laoban.versions.backToCurrent') }}
          </button>
          <button
            v-if="roundPlan && !selectedVersion"
            type="button"
            class="btn btn-sm role-secondary"
            @click="showPlan = !showPlan"
          >
            {{ showPlan ? t('laoban.round.showCurrent') : t('laoban.round.showPlan') }}
          </button>
        </div>
        <p
          v-if="selectedVersion"
          class="laoban-text laoban-hint"
        >
          {{ selectedVersion.summary }}
        </p>
        <div
          v-if="canArrange && pendingCount"
          class="laoban-layout-bar"
        >
          <span>{{ t('laoban.layout.moved', { count: pendingCount }) }}</span>
          <button
            type="button"
            class="btn btn-sm role-secondary"
            @click="discardLayout"
          >
            {{ t('laoban.layout.discard') }}
          </button>
          <button
            type="button"
            class="btn btn-sm role-primary"
            @click="saveLayoutNow"
          >
            {{ t('laoban.layout.save') }}
          </button>
        </div>
        <ArchitectureView
          v-if="view.architecture"
          :architecture="view.architecture"
          :diff="view.diff"
          :positions="canArrange ? pending : null"
          :editable="canArrange"
          :selected="selectedNode"
          @move="onMove"
          @select="selectedNode = $event"
        />
        <StructureEdit
          v-if="canEdit && !selectedCommit"
          :architecture="detail.architecture"
          :selected="selectedNode"
          :disabled="busy || (roundActive && !!roundState && roundState.state !== 'collecting')"
          @add="addStructure"
          @select="selectedNode = $event"
        />
        </div>
      </section>
    </div>

    <StatusBar
      v-if="detail"
      :text="detailStatus"
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

.laoban-lock {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  align-items: center;
  padding: 8px 12px;
  margin-bottom: 12px;
  border: 1px solid var(--border);
  border-left: 4px solid var(--primary);
  border-radius: var(--border-radius);
  font-size: 13px;

  &.laoban-lock-other,
  &.laoban-lock-lost,
  &.laoban-lock-error {
    border-left-color: var(--warning);
  }
}

.laoban-tabs {
  display: flex;
  gap: 4px;
  margin-bottom: 10px;
  border-bottom: 1px solid var(--border);
}

.laoban-tab {
  background: none;
  border: none;
  border-bottom: 3px solid transparent;
  padding: 6px 12px;
  color: var(--body-text);
  font-weight: bold;
  cursor: pointer;

  &.active {
    border-bottom-color: var(--primary);
    color: var(--link);
  }
}

.laoban-arch-panel {
  display: flex;
  flex-direction: column;
  flex: 1 1 auto;
  min-height: 0;
}

.laoban-layout-bar {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  align-items: center;
  margin-bottom: 6px;
  font-size: 12px;

  span {
    margin-right: auto;
  }
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
