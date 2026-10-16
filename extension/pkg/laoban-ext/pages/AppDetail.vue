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
  defineComponent, ref, computed, onMounted, onBeforeUnmount, watch, nextTick
} from 'vue';
import { useStore } from 'vuex';
import { useRoute } from 'vue-router';
import { useText } from '../composables/useText';
import ArchitectureView from '../components/ArchitectureView.vue';
import RoundPanel from '../components/RoundPanel.vue';
import CharCounter from '../components/CharCounter.vue';
import { roundSpace, textState } from '../utils/limits';
import OpinionList from '../components/OpinionList.vue';
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
import { mergeForDiff, diffEntries } from '../utils/graphDiff';
import {
  OpinionDraft, RemovedDraft, opinionItems, opinionCounts, putDraft, removeDraftAt, restoreDraft
} from '../utils/opinions';
import { APPS_ROUTE_NAME, PRODUCT_NAME, BLANK_CLUSTER } from '../config/laoban';
import { formatDateTime } from '../utils/time';
import {
  versionKindKey, previousOf, changedFiles, CompareMode, COMPARE_MODES
} from '../utils/versions';

export default defineComponent({
  name: 'LaobanAppDetail',

  components: {
    ArchitectureView, RoundPanel, OpinionList, StatusBar, VersionList, CodeEditor, StructureEdit, CharCounter
  },

  setup() {
    const store = useStore();
    const route = useRoute();
    const { t } = useText(store);

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
    // What a selected version is compared with: the version before it (what
    // it changed), the current one (what restoring it would change), or
    // nothing.
    const compareMode = ref<CompareMode>('previous');
    // The versions used as the "previous" side, by commit.
    const previousCache = ref<Record<string, Pick<VersionDetail, 'architecture' | 'fileDigests'>>>({});
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
        nextTick(measureStage);
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

    // The 3D view enlarged to the whole window (and optionally full
    // screen), with the versions and the list of changes beside it. It is
    // the same element, so selection, comparison and unsaved positions
    // carry over.
    const expanded = ref(false);
    const archPanel = ref<HTMLElement | null>(null);
    const isFullscreen = ref(false);
    const fullscreenSupported = typeof document !== 'undefined' && !!document.fullscreenEnabled;
    let bodyOverflow = '';

    function onExpandedKey(ev: KeyboardEvent) {
      // In full screen the browser's Esc leaves full screen first.
      if (ev.key === 'Escape' && !document.fullscreenElement) {
        closeExpanded();
      }
    }

    function onFullscreenChange() {
      isFullscreen.value = !!document.fullscreenElement;
    }

    function openExpanded() {
      if (expanded.value) {
        return;
      }
      tab.value = 'arch';
      expanded.value = true;
      bodyOverflow = document.body.style.overflow;
      document.body.style.overflow = 'hidden';
      window.addEventListener('keydown', onExpandedKey);
    }

    function closeExpanded() {
      if (!expanded.value) {
        return;
      }
      if (document.fullscreenElement) {
        document.exitFullscreen?.().catch(() => undefined);
      }
      expanded.value = false;
      document.body.style.overflow = bodyOverflow;
      window.removeEventListener('keydown', onExpandedKey);
    }

    // The 3D view's height on the page: what is left of the window below
    // its top (measured with the page scrolled to the top), between 320 and
    // 520 px, so the whole view is on the first screen.
    const stageHeight = ref(0);

    function measureStage() {
      const el = archPanel.value?.querySelector<HTMLElement>('.laoban-arch-stage');

      if (!el || expanded.value) {
        return;
      }
      const top = el.getBoundingClientRect().top + window.scrollY;

      stageHeight.value = Math.round(Math.max(320, Math.min(520, window.innerHeight - top - 16)));
    }

    function toggleFullscreen() {
      if (document.fullscreenElement) {
        document.exitFullscreen?.().catch(() => undefined);
      } else {
        archPanel.value?.requestFullscreen?.().catch(() => undefined);
      }
    }

    const changes = computed(() => diffEntries(view.value.diff));

    function versionKind(kind: string): string {
      return t(`laoban.versions.kind.${ versionKindKey(kind) }`);
    }

    // The enlarged view's side column: one tab at a time, so the list of
    // changes is never below a long timeline.
    const sideTab = ref<'versions' | 'changes' | 'opinions'>('versions');

    // Picking a version in the timeline shows what it changed.
    function pickTimeline(v: { commit: string; current: boolean }) {
      selectVersion(v.current ? null : v.commit);
      if (compareMode.value !== 'none') {
        sideTab.value = 'changes';
      }
    }

    function pickChange(nodeId: string) {
      selectedNode.value = nodeId;
    }

    // Shown while the page loads a version someone else published.
    const reloading = ref(false);

    async function reloadAll() {
      staleNotice.value = '';
      selectVersion(null);
      pending.value = {};
      editorReload.value++;
      await Promise.all([load(), loadVersions()]);
    }

    async function reloadNewer() {
      reloading.value = true;
      try {
        await reloadAll();
      } finally {
        reloading.value = false;
      }
    }

    const flow = useDeployFlow(store, t, { onReady: reloadAll });
    const round = useRound(store, {
      onApplied:    (applied) => flow.watchApplied(applied),
      messageFor:   flow.messageFor,
      aiTimeoutMs:  () => aiRequestTimeoutMs(flow.config.value),
      lostMessage:  () => t('laoban.round.lost'),
      slowMessage:  () => t('laoban.round.slowContinues'),
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
    const currentNumber = computed(() => versions.value.find((v) => v.current)?.version ?? 0);

    // The version before the one being looked at: a selected version, or in
    // the enlarged view also the current one (what the latest change did).
    const previous = computed(() => {
      if (compareMode.value !== 'previous' || (!selectedCommit.value && !expanded.value)) {
        return null;
      }

      return previousOf(versions.value, selectedCommit.value, versionsTruncated.value);
    });

    watch(previous, async(p) => {
      if (p?.kind !== 'version' || p.entry.commit in previousCache.value) {
        return;
      }
      const commit = p.entry.commit;
      const app = name.value;

      try {
        const v = await getVersion(store, app, commit);

        if (app === name.value) {
          previousCache.value = { ...previousCache.value, [commit]: { architecture: v.architecture, fileDigests: v.fileDigests } };
        }
      } catch (e) {
        versionsError.value = flow.messageFor(e);
      }
    });

    // What the 3D view shows:
    // - a change plan: the change compared with the current version;
    // - a selected earlier version, compared with the version before it
    //   (marks = what that version changed), with the current one (marks =
    //   what restoring it would change), or on its own;
    // - otherwise the current version; in the enlarged view compared with
    //   the version before it.
    const view = computed(() => {
      const current = detail.value?.architecture || null;
      const prev = previous.value;
      // Against the previous version: the diff, or a hint while it loads or
      // when there is nothing before it.
      const againstPrevious = (arch: unknown, digests: Record<string, string> | undefined, n: number) => {
        if (prev?.kind === 'first') {
          const d = mergeForDiff(null, arch as VersionDetail['architecture']);

          return {
            architecture: d.architecture, diff: d, files: null, hint: t('laoban.versions.hintFirst', { n })
          };
        }
        if (prev?.kind === 'version' && prev.entry.commit in previousCache.value) {
          const before = previousCache.value[prev.entry.commit];
          const d = mergeForDiff(before.architecture, arch as VersionDetail['architecture']);

          return {
            architecture: d.architecture, diff: d, files: changedFiles(before.fileDigests, digests), hint: t('laoban.versions.hintPrevious', { n, p: prev.entry.version })
          };
        }

        return null;
      };

      if (showingPlan.value && roundPlan.value && !selectedVersion.value) {
        const d = mergeForDiff(current, roundPlan.value.architecture);
        const same = !d.summary.added && !d.summary.removed && !d.summary.changed;

        return {
          architecture: d.architecture, diff: d, files: null, hint: t(same ? 'laoban.round.hintPlanSame' : 'laoban.round.hintPlan')
        };
      }
      if (selectedVersion.value) {
        if (compareMode.value === 'current') {
          const d = mergeForDiff(current, selectedVersion.value.architecture);

          return {
            architecture: d.architecture, diff: d, files: changedFiles(detail.value?.fileDigests, selectedVersion.value.fileDigests), hint: t('laoban.versions.hintCompare', { n: selectedNumber.value })
          };
        }
        const p = compareMode.value === 'previous' ? againstPrevious(selectedVersion.value.architecture, selectedVersion.value.fileDigests, selectedNumber.value) : null;

        return p || {
          architecture: selectedVersion.value.architecture, diff: null, files: null, hint: t('laoban.versions.hintVersion', { n: selectedNumber.value })
        };
      }
      const p = expanded.value && compareMode.value === 'previous' && current ? againstPrevious(current, detail.value?.fileDigests, currentNumber.value) : null;

      return p || {
        architecture: current, diff: null, files: null, hint: t('laoban.edit.currentArch')
      };
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
        reloadNewer();
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
        reloadNewer();
      }
    }

    function addStructure(line: string) {
      opinionDraft.value = appendLine(opinionDraft.value, line, 1000);
    }

    // Opinions on components: one draft per component, written in the 3D
    // view, sent together with the general opinion.
    const drafts = ref<OpinionDraft[]>([]);
    const componentText = ref('');
    // Opinions can be written when no round runs (they open one) or while
    // one collects; on the current version only (node ids must exist now).
    const canComment = computed(() => !!detail.value && !selectedCommit.value &&
      (!round.active.value || round.round.value?.state === 'collecting'));
    const selectedNodeName = computed(() => {
      const nodes = (detail.value?.architecture as { nodes?: { id: string; name?: string }[] } | null)?.nodes || [];
      const n = nodes.find((x) => x.id === selectedNode.value);

      return n ? (n.name || n.id) : '';
    });
    const selectedDraft = computed(() => drafts.value.find((d) => d.target === selectedNode.value) || null);
    // Size limits of rounds, as the backend reports them; too long text is
    // kept as typed and cannot be saved or sent.
    const roundLimits = computed(() => round.limits.value);
    const roundsAvailable = computed(() => round.available.value);
    const componentTooLong = computed(() => textState(componentText.value, roundLimits.value.opinionChars).over);
    const roundLeft = computed(() => roundSpace(round.round.value, roundLimits.value, { chars: 0, opinions: 0 }).left);
    const badges = computed(() => (round.active.value ? opinionCounts(round.round.value?.opinions) : null));
    // The enlarged view's Opinions tab: while a round runs or drafts wait.
    const showOpinionsTab = computed(() => (!!round.round.value && round.active.value) || drafts.value.length > 0);

    watch(showOpinionsTab, (on) => {
      if (!on && sideTab.value === 'opinions') {
        sideTab.value = 'versions';
      }
    });

    // "Added" next to the button after a draft is saved (report 18: the
    // draft list is far away, at the top of the page).
    const draftSaved = ref(false);
    // The last removed draft, for "undo".
    const removedDraft = ref<RemovedDraft | null>(null);

    watch(selectedNode, () => {
      componentText.value = selectedDraft.value?.text || '';
      draftSaved.value = false;
    });
    watch(componentText, () => {
      draftSaved.value = false;
    });

    // Every component of the view, for the picker above the 3D view (labels
    // can sit on top of each other there).
    const nodeOptions = computed(() => {
      const nodes = (view.value.architecture as { nodes?: { id: string; name?: string; layer?: string }[] } | null)?.nodes || [];

      return nodes
        .filter((n) => n && typeof n.id === 'string')
        .map((n) => ({ id: n.id, label: `${ n.layer ? `${ t(`laoban.dashboard.arch.layers.${ n.layer }`) } · ` : '' }${ n.name || n.id }` }))
        .sort((a, b) => a.label.localeCompare(b.label));
    });

    // A component picked (in the 3D view or the picker): bring the opinion
    // box into sight, so it is clear something happened.
    const composer = ref<HTMLElement | null>(null);

    function pickNode(id: string | null) {
      selectedNode.value = id || null;
      if (id && canComment.value) {
        nextTick(() => composer.value?.scrollIntoView?.({ block: 'nearest', behavior: 'smooth' }));
      }
    }

    function saveDraft() {
      if (selectedNode.value && selectedNodeName.value && componentText.value.trim() && !componentTooLong.value) {
        drafts.value = putDraft(drafts.value, { target: selectedNode.value, targetName: selectedNodeName.value, text: componentText.value });
        removedDraft.value = null;
        // Set after the text watcher has run.
        nextTick(() => {
          draftSaved.value = true;
        });
      }
    }

    function removeDraft(target: string) {
      const { drafts: rest, removed } = removeDraftAt(drafts.value, target);

      drafts.value = rest;
      removedDraft.value = removed;
      if (target === selectedNode.value) {
        componentText.value = '';
      }
    }

    function undoRemoveDraft() {
      const r = removedDraft.value;

      if (r) {
        drafts.value = restoreDraft(drafts.value, r);
        removedDraft.value = null;
        if (r.draft.target === selectedNode.value) {
          componentText.value = r.draft.text;
        }
      }
    }

    function selectTarget(id: string) {
      selectedNode.value = id;
      if (!expanded.value) {
        tab.value = 'arch';
      }
    }

    function clearOpinions() {
      opinionDraft.value = '';
      drafts.value = [];
      componentText.value = '';
      removedDraft.value = null;
      draftSaved.value = false;
    }

    async function openRound(opts: { minutes: number; scope: string }) {
      if (await round.open(opinionItems(opinionDraft.value, drafts.value), opts.minutes, opts.scope)) {
        clearOpinions();
      }
    }

    async function addOpinions() {
      if (await round.add(opinionItems(opinionDraft.value, drafts.value))) {
        clearOpinions();
      }
    }

    async function roundAct(action: RoundAction, body: Record<string, string>) {
      if (await round.act(action, body) && action === 'opinions') {
        opinionDraft.value = '';
      }
    }

    // The deploy's progress, shown under the round as well.
    const deployNotice = computed(() => (flow.phase.value === 'deploying' ? flow.statusText.value : ''));
    const roundNotice = computed(() => round.notice.value || deployNotice.value);
    // Someone else's new version: a notice fixed on screen, seen wherever
    // the page is scrolled.
    const toast = computed(() => (reloading.value ? t('laoban.detail.reloading') : staleNotice.value));
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
          reloadNewer();
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
      clearOpinions();
      selectedNode.value = null;
      tab.value = 'arch';
      previousCache.value = {};
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
      document.addEventListener('fullscreenchange', onFullscreenChange);
      window.addEventListener('resize', measureStage);
      versionTimer = setInterval(checkVersions, VERSION_POLL_MS);
    });

    onBeforeUnmount(() => {
      closeExpanded();
      document.removeEventListener('fullscreenchange', onFullscreenChange);
      window.removeEventListener('resize', measureStage);
      window.removeEventListener('beforeunload', beforeUnload);
      if (versionTimer) {
        clearInterval(versionTimer);
      }
    });

    return {
      t,
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
      compareMode,
      compareModes: COMPARE_MODES,
      sideTab,
      showOpinionsTab,
      pickTimeline,
      pickChange,
      toast,
      stageHeight,
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
      roundLimits,
      roundsAvailable,
      roundLeft,
      componentTooLong,
      detailStatus,
      expanded,
      archPanel,
      isFullscreen,
      fullscreenSupported,
      openExpanded,
      closeExpanded,
      toggleFullscreen,
      changes,
      versionKind,
      roundState: round.round,
      roundActive: round.active,
      roundPending: round.pending,
      roundElapsed: round.elapsed,
      roundError: round.error,
      openRound,
      roundAct,
      drafts,
      componentText,
      canComment,
      selectedNodeName,
      selectedDraft,
      badges,
      saveDraft,
      removeDraft,
      undoRemoveDraft,
      removedDraft,
      draftSaved,
      nodeOptions,
      pickNode,
      composer,
      selectTarget,
      addOpinions,
      roundSupport: round.support,
      roundReply: round.reply,
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
  <div class="laoban-m0">
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
          :limits="roundLimits"
          :available="roundsAvailable"
          :drafts="drafts"
          :removed-draft="removedDraft"
          :selected-target="selectedNode"
          @open="openRound"
          @act="roundAct"
          @add="addOpinions"
          @remove-draft="removeDraft"
          @undo-draft="undoRemoveDraft"
          @support="roundSupport"
          @reply="roundReply"
          @select-target="selectTarget"
        />

        <p
          v-if="errorText"
          class="laoban-error text-error"
          role="alert"
        >
          {{ errorText }}
        </p>

        <details class="laoban-detail-section">
          <summary>{{ t('laoban.detail.currentSteps') }}（{{ currentSteps.length }}）</summary>
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
          ref="archPanel"
          role="tabpanel"
          class="laoban-arch-panel"
          :class="{ 'laoban-arch-expanded': expanded }"
          :style="stageHeight ? { '--laoban-stage-h': `${ stageHeight }px` } : null"
        >
        <div class="laoban-arch-main">
        <div class="laoban-view-head">
          <p class="text-muted laoban-hint">
            {{ view.hint }}
          </p>
          <label
            v-if="nodeOptions.length"
            class="laoban-compare"
          >
            {{ t('laoban.opinions.pick') }}
            <select
              class="laoban-compare-select laoban-node-picker"
              :value="selectedNode || ''"
              @change="pickNode($event.target.value)"
            >
              <option value="">{{ t('laoban.opinions.pickNone') }}</option>
              <option
                v-for="o in nodeOptions"
                :key="o.id"
                :value="o.id"
              >{{ o.label }}</option>
            </select>
          </label>
          <label
            v-if="selectedVersion"
            class="laoban-compare"
          >
            {{ t('laoban.versions.compareWith') }}
            <select
              v-model="compareMode"
              class="laoban-compare-select"
            >
              <option
                v-for="m in compareModes"
                :key="m"
                :value="m"
              >{{ t(`laoban.versions.compareModes.${ m }`) }}</option>
            </select>
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
          :badges="badges"
          @move="onMove"
          @select="pickNode($event)"
        >
          <template #tools>
            <button
              v-if="expanded && fullscreenSupported"
              type="button"
              class="btn btn-sm role-secondary"
              @click="toggleFullscreen"
            >
              {{ isFullscreen ? t('laoban.expand.exitFullscreen') : t('laoban.expand.fullscreen') }}
            </button>
            <button
              type="button"
              class="btn btn-sm"
              :class="expanded ? 'role-primary' : 'role-secondary'"
              :aria-pressed="expanded ? 'true' : 'false'"
              @click="expanded ? closeExpanded() : openExpanded()"
            >
              {{ expanded ? t('laoban.expand.close') : t('laoban.expand.open') }}
            </button>
          </template>
        </ArchitectureView>
        <form
          v-if="canComment && selectedNode && selectedNodeName"
          ref="composer"
          class="laoban-component-opinion"
          @submit.prevent="saveDraft"
        >
          <label
            for="laoban-component-opinion"
            class="laoban-label"
          >{{ t('laoban.opinions.onComponent', { name: selectedNodeName }) }}</label>
          <textarea
            id="laoban-component-opinion"
            v-model="componentText"
            rows="2"
            :class="{ 'laoban-over': componentTooLong }"
            :aria-invalid="componentTooLong ? 'true' : 'false'"
            :placeholder="t('laoban.opinions.componentPlaceholder')"
          />
          <CharCounter
            :text="componentText"
            :max="roundLimits.opinionChars"
          />
          <div class="laoban-component-opinion-actions">
            <button
              type="submit"
              class="btn btn-sm role-primary"
              :disabled="!componentText.trim() || componentTooLong"
            >
              {{ selectedDraft ? t('laoban.opinions.updateDraft') : t('laoban.opinions.addDraft') }}
            </button>
            <button
              v-if="selectedDraft"
              type="button"
              class="btn btn-sm role-secondary"
              @click="removeDraft(selectedNode)"
            >
              {{ t('laoban.opinions.removeDraftShort') }}
            </button>
            <span
              v-if="draftSaved"
              class="laoban-draft-saved"
              role="status"
            >✓ {{ t('laoban.opinions.draftSaved', { count: drafts.length }) }}</span>
            <span
              v-else
              class="text-muted laoban-hint"
            >{{ t('laoban.opinions.draftHint', { count: drafts.length }) }}</span>
          </div>
        </form>
        <StructureEdit
          v-if="canEdit && !selectedCommit"
          :architecture="detail.architecture"
          :selected="selectedNode"
          :disabled="busy || (roundActive && !!roundState && roundState.state !== 'collecting')"
          @add="addStructure"
          @select="selectedNode = $event"
        />
        </div>
        <aside
          v-if="expanded"
          class="laoban-arch-side"
        >
          <div
            class="laoban-side-tabs"
            role="tablist"
          >
            <button
              v-for="st in (showOpinionsTab ? ['versions', 'changes', 'opinions'] : ['versions', 'changes'])"
              :key="st"
              type="button"
              role="tab"
              class="laoban-side-tab"
              :class="{ selected: sideTab === st }"
              :aria-selected="sideTab === st ? 'true' : 'false'"
              @click="sideTab = st"
            >
              {{ t(`laoban.expand.tabs.${ st }`) }}<template v-if="st === 'changes' && view.diff">
                （{{ changes.length + (view.files ? view.files.length : 0) }}）
              </template><template v-if="st === 'opinions' && roundState && roundActive">
                （{{ roundState.opinions.length }}）
              </template>
            </button>
          </div>
          <label class="laoban-compare">
            {{ t('laoban.versions.compareWith') }}
            <select
              v-model="compareMode"
              class="laoban-compare-select"
            >
              <option
                v-for="m in compareModes"
                :key="m"
                :value="m"
              >{{ t(`laoban.versions.compareModes.${ m }`) }}</option>
            </select>
          </label>
          <section
            v-if="sideTab === 'versions'"
            role="tabpanel"
            class="laoban-side-panel"
          >
            <ol class="laoban-timeline">
              <li
                v-for="v in versions"
                :key="v.commit"
              >
                <button
                  type="button"
                  class="laoban-timeline-item"
                  :class="{ selected: selectedCommit === v.commit || (!selectedCommit && v.current) }"
                  :aria-current="(selectedCommit === v.commit || (!selectedCommit && v.current)) ? 'true' : undefined"
                  :disabled="busy"
                  @click="pickTimeline(v)"
                >
                  <span class="laoban-timeline-line">
                    <strong>{{ t('laoban.versions.number', { n: v.version }) }}</strong>
                    <span
                      v-if="v.current"
                      class="badge-state bg-success"
                    >{{ t('laoban.versions.current') }}</span>
                    <span class="text-muted">{{ formatTime(v.at) }} · {{ versionKind(v.kind) }}</span>
                  </span>
                  <span
                    v-if="v.request"
                    class="laoban-timeline-request laoban-text"
                    :title="v.request"
                  >{{ v.request }}</span>
                </button>
              </li>
            </ol>
          </section>
          <section
            v-else-if="sideTab === 'changes'"
            role="tabpanel"
            class="laoban-side-panel"
          >
            <p class="text-muted laoban-hint">
              {{ view.hint }}
            </p>
            <p
              v-if="!view.diff"
              class="text-muted laoban-hint"
            >
              {{ t(compareMode === 'none' ? 'laoban.expand.noComparisonOff' : 'laoban.expand.noComparison') }}
            </p>
            <template v-else>
              <div
                v-if="view.files && view.files.length"
                class="laoban-file-changes"
              >
                <span class="laoban-label">{{ t('laoban.expand.filesChanged', { count: view.files.length }) }}</span>
                <ul>
                  <li
                    v-for="f in view.files"
                    :key="f"
                  >
                    <code>{{ f }}</code>
                  </li>
                </ul>
              </div>
              <p
                v-if="!changes.length"
                class="text-muted laoban-hint"
              >
                {{ t(view.files ? (view.files.length ? 'laoban.expand.noArchChanges' : 'laoban.expand.noChangesAtAll') : 'laoban.expand.noChanges') }}
              </p>
            </template>
            <ul
              v-if="view.diff && changes.length"
              class="laoban-changes"
            >
              <li
                v-for="c in changes"
                :key="c.type + c.id"
              >
                <button
                  type="button"
                  class="laoban-change"
                  :class="[`laoban-change-${ c.status }`, { selected: selectedNode === c.nodeId }]"
                  @click="pickChange(c.nodeId)"
                >
                  <span class="laoban-change-status">{{ t(`laoban.expand.status.${ c.status }`) }}</span>
                  <span class="laoban-text">{{ c.label }}</span>
                  <span class="text-muted">{{ c.type === 'edge' ? t('laoban.expand.connection') : c.kind }}</span>
                  <span
                    v-if="c.file"
                    class="text-muted laoban-change-file"
                  >{{ t(`laoban.expand.file.${ c.file }`) }}</span>
                </button>
              </li>
            </ul>
          </section>
          <section
            v-else
            role="tabpanel"
            class="laoban-side-panel"
          >
            <OpinionList
              v-if="roundState && roundActive"
              :opinions="roundState.opinions"
              :me="roundState.you ? roundState.you.user : ''"
              :can-discuss="!!roundState.you && !!roundState.you.canDiscuss"
              :busy="busy"
              :selected="selectedNode"
              :limits="roundLimits"
              :round-left="roundLeft"
              @support="roundSupport"
              @reply="roundReply"
              @select="selectTarget"
            />
            <template v-if="drafts.length">
              <p class="laoban-hint">
                {{ t('laoban.opinions.drafts', { count: drafts.length }) }}：{{ drafts.map((d) => d.targetName).join('、') }}
              </p>
              <button
                v-if="roundState && roundState.state === 'collecting'"
                type="button"
                class="btn btn-sm role-primary"
                :disabled="busy"
                @click="addOpinions"
              >
                {{ t('laoban.opinions.sendMany', { count: drafts.length + (opinionDraft.trim() ? 1 : 0) }) }}
              </button>
              <p
                v-else
                class="text-muted laoban-hint"
              >
                {{ t('laoban.opinions.sideOpenHint') }}
              </p>
            </template>
          </section>
        </aside>
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
    <div
      v-if="toast"
      class="laoban-toast"
      role="status"
      aria-live="polite"
    >
      {{ toast }}
    </div>
  </div>
</template>

<style lang="scss" scoped>
@import '../styles/layout.scss';


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

  // A fixed height: the left column (versions, steps) must not stretch the
  // 3D view, or its components end up out of sight.
  :deep(.laoban-arch-stage) {
    flex: none;
    height: var(--laoban-stage-h, min(520px, 70vh));
    min-height: 320px;
  }
}

// The right column keeps its own height instead of the left one's.
.laoban-view {
  align-self: start;
}

.laoban-arch-main {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}

.laoban-component-opinion {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-top: 8px;
  padding: 8px;
  border: 1px solid var(--border);
  border-left: 3px solid var(--primary);
  border-radius: var(--border-radius);

  textarea {
    width: 100%;
  }
}

.laoban-draft-saved {
  color: var(--success);
  font-size: 12px;
}

.laoban-node-picker {
  max-width: 260px;
}

.laoban-component-opinion-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
}

// Enlarged: the panel covers the window (the same element, so the 3D view
// keeps its state); versions and changes on the right.
.laoban-arch-panel.laoban-arch-expanded {
  position: fixed;
  inset: 0;
  z-index: 1000;
  display: grid;
  grid-template-columns: minmax(0, 1fr) 320px;
  gap: 16px;
  padding: 16px;
  background: var(--body-bg);
  overflow: hidden;

  // The 3D view takes what the column has left, so the opinion box under
  // it stays in sight (report 18: only its edge showed).
  .laoban-arch-main {
    overflow-y: auto;
    height: 100%;
  }

  :deep(.laoban-arch) {
    flex: 1 1 auto;
    min-height: 0;
  }

  :deep(.laoban-arch-stage) {
    height: auto;
    flex: 1 1 auto;
    min-height: 300px;
  }

  .laoban-component-opinion {
    flex: none;
  }

  @media (max-width: 900px) {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: minmax(0, 1fr) 40vh;
  }
}

.laoban-arch-side {
  display: flex;
  flex-direction: column;
  gap: 10px;
  min-height: 0;
  overflow: hidden;
  padding-left: 16px;
  border-left: 1px solid var(--border);

  h3 {
    margin: 0 0 8px;
  }
}

.laoban-timeline,
.laoban-changes {
  list-style: none;
  padding: 0;
  margin: 8px 0 0;
}

.laoban-timeline-item,
.laoban-change {
  display: flex;
  flex-wrap: wrap;
  gap: 2px 8px;
  align-items: baseline;
  width: 100%;
  padding: 6px 8px;
  border: none;
  border-left: 3px solid transparent;
  background: none;
  color: var(--body-text);
  text-align: left;
  cursor: pointer;

  &:hover:not(:disabled) {
    background: var(--sortable-table-hover-bg, var(--box-bg));
  }

  &.selected {
    border-left-color: var(--primary);
    background: var(--sortable-table-hover-bg, var(--box-bg));
  }
}

// Two lines per version: number, time and kind; then the request.
.laoban-timeline-item {
  flex-direction: column;
  align-items: stretch;
  padding: 4px 8px;
}

.laoban-timeline-line {
  display: flex;
  flex-wrap: wrap;
  gap: 2px 8px;
  align-items: baseline;
  font-size: 13px;
}

.laoban-timeline-request {
  font-size: 12px;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.laoban-side-tabs {
  display: flex;
  gap: 4px;
  border-bottom: 1px solid var(--border);
  flex: none;
}

.laoban-side-tab {
  padding: 6px 10px;
  border: none;
  border-bottom: 3px solid transparent;
  background: none;
  color: var(--body-text);
  cursor: pointer;

  &.selected {
    border-bottom-color: var(--primary);
    font-weight: bold;
  }
}

.laoban-side-panel {
  padding-right: 4px;
  min-height: 0;
  overflow-y: auto;
}

.laoban-file-changes {
  margin-bottom: 8px;
  font-size: 13px;

  ul {
    list-style: none;
    padding: 0;
    margin: 4px 0 0;
  }

  li {
    padding: 2px 8px;
    border-left: 3px solid var(--laoban-diff-changed, var(--warning));
  }
}

.laoban-change-file {
  flex-basis: 100%;
  font-size: 12px;
  padding-left: calc(3em + 8px);
}

.laoban-compare-select {
  width: auto;
  margin-left: 6px;
  padding: 2px 6px;
}

// Fixed on screen: seen wherever the page is scrolled.
.laoban-toast {
  position: fixed;
  right: 24px;
  bottom: 24px;
  z-index: 1001;
  max-width: min(420px, calc(100vw - 48px));
  padding: 10px 14px;
  border-left: 4px solid var(--primary);
  border-radius: var(--border-radius);
  background: var(--body-bg);
  color: var(--body-text);
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.25);
}

.laoban-change-status {
  font-weight: bold;
  min-width: 3em;
}

.laoban-change-added .laoban-change-status {
  color: var(--laoban-diff-added, var(--success));
}

.laoban-change-removed .laoban-change-status {
  color: var(--laoban-diff-removed, var(--error));
}

.laoban-change-changed .laoban-change-status {
  color: var(--laoban-diff-changed, var(--warning));
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

<style lang="scss">
@import '../styles/global.scss';
.laoban-over {
  border-color: var(--error) !important;
}
</style>
