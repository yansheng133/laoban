<script lang="ts">
// M0 page (docs/m0-design.md, "頁面（Rancher UI extension）"):
// left: requirement -> plan, right: 3D architecture, bottom: status bar.
// "確認並部署" commits through the backend, then Fleet bundle readiness is
// polled through Steve until the app link can be shown.
//
// Rules for this page: every visible string goes through t(), and all text
// that comes from the backend or AI is rendered with {{ }} interpolation,
// never v-html. See extension/README.md.
import {
  defineComponent, ref, computed, onMounted, onBeforeUnmount
} from 'vue';
import { useStore } from 'vuex';
import { useI18n } from '@shell/composables/useI18n';
import ArchitectureView from '../components/ArchitectureView.vue';
import AppList from '../components/AppList.vue';
import {
  getConfig, createPlan, applyPlan, listBundles, resolveAppUrl, safeHttpsUrl, aiRequestTimeoutMs, getApp,
  RequestError, ConfigResponse, PlanResponse, ApplyResponse, AppDetail
} from '../utils/backend';
import { bundleReadiness, Readiness } from '../utils/bundle';
import {
  APP_NAME_PATTERN, BUNDLE_POLL_INTERVAL_MS, BUNDLE_POLL_TIMEOUT_MS, FLEET_WORKSPACE
} from '../config/backend';

type Phase = 'idle' | 'planning' | 'planned' | 'applying' | 'deploying' | 'ready' | 'timeout';

export default defineComponent({
  name: 'LaobanDashboard',

  components: { ArchitectureView, AppList },

  setup() {
    const store = useStore();
    const { t } = useI18n(store);

    const config = ref<ConfigResponse | null>(null);
    const configError = ref('');

    const prompt = ref('');
    const appName = ref('');
    const phase = ref<Phase>('idle');
    const errorText = ref('');
    const plan = ref<PlanResponse | null>(null);
    const applied = ref<ApplyResponse | null>(null);
    const readiness = ref<Readiness | null>(null);
    const pollError = ref(false);
    const elapsed = ref(0);
    // Set while an existing app is being changed ("編輯" in the app list).
    const editing = ref<AppDetail | null>(null);
    const loadingApp = ref('');
    const appList = ref<{ refresh: () => Promise<void> } | null>(null);
    const workArea = ref<HTMLElement | null>(null);

    let elapsedTimer: ReturnType<typeof setInterval> | null = null;
    let pollTimer: ReturnType<typeof setTimeout> | null = null;
    let pollStarted = 0;
    let pollGeneration = 0;
    let unmounted = false;

    const busy = computed(() => phase.value === 'planning' || phase.value === 'applying' || !!loadingApp.value);
    const appNameInvalid = computed(() => !editing.value && !!appName.value.trim() && !APP_NAME_PATTERN.test(appName.value.trim()));
    // The 3D view shows the new plan, or the app being edited before there is one.
    const shownArchitecture = computed(() => plan.value?.architecture || editing.value?.architecture || null);
    const recentHistory = computed(() => (editing.value?.history || []).slice(0, 5));
    const canPlan = computed(() => !busy.value && !!prompt.value.trim() && !appNameInvalid.value);
    const canApply = computed(() => !busy.value && !!plan.value && !applied.value);
    const commitLink = computed(() => safeHttpsUrl(applied.value?.commitUrl));
    const shortCommit = computed(() => String(applied.value?.commit || '').slice(0, 12));
    const appLink = computed(() => resolveAppUrl(applied.value?.appUrl, window.location.origin));
    const steps = computed(() => {
      const list = plan.value?.steps;

      return Array.isArray(list) ? list.map((s) => String(s)) : [];
    });

    function messageFor(e: unknown): string {
      if (e instanceof RequestError && e.userMessage) {
        return e.userMessage;
      }
      if (e instanceof RequestError && e.code === 'timeout') {
        return t('laoban.dashboard.errors.timeout');
      }

      return t('laoban.dashboard.errors.generic');
    }

    function startElapsed() {
      stopElapsed();
      elapsed.value = 0;
      const start = Date.now();

      elapsedTimer = setInterval(() => {
        elapsed.value = Math.floor((Date.now() - start) / 1000);
      }, 1000);
    }

    function stopElapsed() {
      if (elapsedTimer) {
        clearInterval(elapsedTimer);
        elapsedTimer = null;
      }
    }

    function stopPolling() {
      pollGeneration++;
      if (pollTimer) {
        clearTimeout(pollTimer);
        pollTimer = null;
      }
    }

    async function loadConfig() {
      try {
        config.value = await getConfig(store);
        configError.value = '';
      } catch (e) {
        configError.value = messageFor(e);
      }
    }

    async function poll(gen: number) {
      if (unmounted || gen !== pollGeneration || !applied.value) {
        return;
      }
      try {
        const bundles = await listBundles(store);

        if (gen !== pollGeneration) {
          return;
        }
        readiness.value = bundleReadiness(bundles, applied.value.fleet?.bundlePrefix || '', applied.value.commit);
        pollError.value = false;
      } catch (e) {
        if (gen !== pollGeneration) {
          return;
        }
        pollError.value = true;
      }

      // 'modified': running but drifted from Git; offer the app anyway.
      if (readiness.value?.state === 'ready' || readiness.value?.state === 'modified') {
        phase.value = 'ready';
        stopElapsed();
        appList.value?.refresh();

        return;
      }
      if (Date.now() - pollStarted >= BUNDLE_POLL_TIMEOUT_MS) {
        phase.value = 'timeout';
        stopElapsed();

        return;
      }
      pollTimer = setTimeout(() => poll(gen), BUNDLE_POLL_INTERVAL_MS);
    }

    function startPolling() {
      stopPolling();
      readiness.value = null;
      pollError.value = false;
      pollStarted = Date.now();
      phase.value = 'deploying';
      startElapsed();
      poll(pollGeneration);
    }

    function formatTime(at: string): string {
      const d = new Date(at);

      return Number.isNaN(d.getTime()) ? at : d.toLocaleString();
    }

    function resetWork() {
      stopPolling();
      stopElapsed();
      errorText.value = '';
      plan.value = null;
      applied.value = null;
      readiness.value = null;
      phase.value = 'idle';
    }

    async function startEdit(name: string) {
      if (busy.value) {
        return;
      }
      resetWork();
      loadingApp.value = name;
      try {
        editing.value = await getApp(store, name);
        appName.value = editing.value.appName;
        prompt.value = '';
        workArea.value?.scrollIntoView?.({ behavior: 'smooth', block: 'start' });
      } catch (e) {
        errorText.value = messageFor(e);
      } finally {
        loadingApp.value = '';
      }
    }

    function cancelEdit() {
      if (busy.value) {
        return;
      }
      resetWork();
      editing.value = null;
      appName.value = '';
      prompt.value = '';
    }

    async function generatePlan() {
      if (!canPlan.value) {
        return;
      }
      stopPolling();
      errorText.value = '';
      plan.value = null;
      applied.value = null;
      readiness.value = null;
      phase.value = 'planning';
      startElapsed();
      try {
        plan.value = await createPlan(
          store,
          prompt.value.trim(),
          appName.value.trim() || undefined,
          aiRequestTimeoutMs(config.value),
          editing.value?.appName
        );
        phase.value = 'planned';
      } catch (e) {
        errorText.value = messageFor(e);
        phase.value = 'idle';
      } finally {
        stopElapsed();
        loadConfig();
      }
    }

    async function confirmAndDeploy() {
      if (!canApply.value || !plan.value) {
        return;
      }
      errorText.value = '';
      phase.value = 'applying';
      startElapsed();
      try {
        applied.value = await applyPlan(store, plan.value.planId, aiRequestTimeoutMs(config.value));
      } catch (e) {
        errorText.value = messageFor(e);
        phase.value = 'planned';
        stopElapsed();

        return;
      } finally {
        loadConfig();
      }
      appList.value?.refresh();
      startPolling();
    }

    const statusText = computed(() => {
      switch (phase.value) {
      case 'planning':
        return t('laoban.dashboard.status.planning', { seconds: elapsed.value });
      case 'planned':
        return t('laoban.dashboard.status.planned');
      case 'applying':
        return t('laoban.dashboard.status.applying', { seconds: elapsed.value });
      case 'deploying': {
        const r = readiness.value;

        if (!r || r.state === 'missing') {
          return t('laoban.dashboard.status.waitingBundle', { seconds: elapsed.value });
        }
        if (r.state === 'error') {
          return t('laoban.dashboard.status.bundleError', { ready: r.ready, desired: r.desired });
        }

        return t('laoban.dashboard.status.deploying', { ready: r.ready, desired: r.desired });
      }
      case 'ready':
        return readiness.value?.state === 'modified' ? t('laoban.dashboard.status.readyModified', { workspace: FLEET_WORKSPACE }) : t('laoban.dashboard.status.ready');
      case 'timeout':
        return t('laoban.dashboard.status.timeout', { minutes: Math.round(BUNDLE_POLL_TIMEOUT_MS / 60000), workspace: FLEET_WORKSPACE });
      default:
        return t('laoban.dashboard.status.idle');
      }
    });

    const formatNumber = (n: unknown) => (typeof n === 'number' && Number.isFinite(n) ? n.toLocaleString() : '-');

    onMounted(loadConfig);

    onBeforeUnmount(() => {
      unmounted = true;
      stopPolling();
      stopElapsed();
    });

    return {
      t,
      config,
      configError,
      prompt,
      appName,
      phase,
      errorText,
      plan,
      steps,
      applied,
      readiness,
      pollError,
      busy,
      appNameInvalid,
      canPlan,
      canApply,
      commitLink,
      shortCommit,
      appLink,
      statusText,
      formatNumber,
      generatePlan,
      confirmAndDeploy,
      editing,
      loadingApp,
      appList,
      workArea,
      shownArchitecture,
      recentHistory,
      formatTime,
      startEdit,
      cancelEdit,
    };
  },
});
</script>

<template>
  <div class="laoban-m0">
    <header class="laoban-header">
      <div>
        <h1>{{ t('laoban.dashboard.title') }}</h1>
        <p class="text-muted">
          {{ t('laoban.dashboard.description') }}
        </p>
      </div>
      <dl
        v-if="config"
        class="laoban-config"
        data-testid="laoban-config"
      >
        <dt>{{ t('laoban.dashboard.config.model') }}</dt>
        <dd>{{ config.model }}</dd>
        <dt>{{ t('laoban.dashboard.config.targetNamespace') }}</dt>
        <dd>{{ config.targetNamespace }}</dd>
        <dt>{{ t('laoban.dashboard.config.repo') }}</dt>
        <dd>{{ config.repo }}</dd>
        <dt>{{ t('laoban.dashboard.config.tokenBudget') }}</dt>
        <dd>{{ t('laoban.dashboard.config.tokenUsage', { used: formatNumber(config.tokenBudget && config.tokenBudget.used), cap: formatNumber(config.tokenBudget && config.tokenBudget.cap) }) }}</dd>
      </dl>
      <p
        v-else-if="configError"
        class="laoban-config text-warning"
      >
        {{ t('laoban.dashboard.config.unavailable') }} {{ configError }}
      </p>
    </header>

    <div
      ref="workArea"
      class="laoban-work"
    >
      <section
        class="laoban-panel laoban-chat"
        :aria-label="t('laoban.dashboard.chat.title')"
      >
        <h2>{{ editing ? t('laoban.edit.title', { appName: editing.appName }) : t('laoban.dashboard.chat.title') }}</h2>
        <div
          v-if="editing"
          class="laoban-editing"
          data-testid="laoban-editing"
        >
          <p class="laoban-text">
            {{ editing.summary }}
          </p>
          <template v-if="recentHistory.length">
            <h4>{{ t('laoban.edit.history') }}</h4>
            <ul class="laoban-history">
              <li
                v-for="(h, i) in recentHistory"
                :key="i"
              >
                <span class="text-muted">{{ formatTime(h.at) }} · {{ h.kind === 'edit' ? t('laoban.edit.kindEdit') : t('laoban.edit.kindCreate') }}</span>
                <span class="laoban-text">{{ h.request }}</span>
              </li>
            </ul>
          </template>
          <button
            type="button"
            class="btn btn-sm role-secondary"
            :disabled="busy"
            @click="cancelEdit"
          >
            {{ t('laoban.edit.cancel') }}
          </button>
        </div>
        <form @submit.prevent="generatePlan">
          <label
            for="laoban-prompt"
            class="laoban-label"
          >{{ editing ? t('laoban.edit.promptLabel') : t('laoban.dashboard.chat.promptLabel') }}</label>
          <textarea
            id="laoban-prompt"
            v-model="prompt"
            rows="5"
            maxlength="4000"
            :placeholder="editing ? t('laoban.edit.promptPlaceholder') : t('laoban.dashboard.chat.promptPlaceholder')"
            :disabled="busy"
          />
          <template v-if="!editing">
            <label
              for="laoban-app-name"
              class="laoban-label"
            >{{ t('laoban.dashboard.chat.appNameLabel') }}</label>
            <input
              id="laoban-app-name"
              v-model="appName"
              type="text"
              maxlength="31"
              autocomplete="off"
              spellcheck="false"
              :placeholder="t('laoban.dashboard.chat.appNamePlaceholder')"
              :disabled="busy"
              :aria-invalid="appNameInvalid"
            >
            <p
              class="laoban-hint"
              :class="{ 'text-error': appNameInvalid, 'text-muted': !appNameInvalid }"
            >
              {{ t('laoban.dashboard.chat.appNameHint') }}
            </p>
          </template>
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
            {{ editing ? t('laoban.edit.generate') : t('laoban.dashboard.chat.generate') }}
          </button>
        </form>

        <p
          v-if="errorText"
          class="laoban-error text-error"
          role="alert"
        >
          {{ errorText }}
        </p>

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
            @click="confirmAndDeploy"
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
          <p v-if="phase === 'ready'">
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
          </p>
        </div>
      </section>

      <section
        class="laoban-panel laoban-view"
        :aria-label="t('laoban.dashboard.arch.title')"
      >
        <h2>{{ t('laoban.dashboard.arch.title') }}</h2>
        <p
          v-if="editing && !plan"
          class="text-muted laoban-hint"
        >
          {{ t('laoban.edit.currentArch') }}
        </p>
        <ArchitectureView
          v-if="shownArchitecture"
          :architecture="shownArchitecture"
        />
        <p
          v-else
          class="laoban-empty text-muted"
        >
          {{ t('laoban.dashboard.arch.empty') }}
        </p>
      </section>
    </div>

    <AppList
      ref="appList"
      :editing-app="editing ? editing.appName : null"
      :busy="busy"
      @edit="startEdit"
    />

    <footer
      class="laoban-status"
      role="status"
      aria-live="polite"
    >
      <i
        v-if="busy || phase === 'deploying'"
        class="icon icon-spinner icon-spin"
        aria-hidden="true"
      />
      <span>{{ statusText }}</span>
      <span
        v-if="(phase === 'deploying' || (phase === 'ready' && readiness && readiness.state === 'modified')) && readiness && readiness.message"
        class="text-muted laoban-text"
      >{{ readiness.message }}</span>
      <span
        v-if="phase === 'deploying' && pollError"
        class="text-warning"
      >{{ t('laoban.dashboard.status.pollFailed') }}</span>
    </footer>
  </div>
</template>

<style lang="scss" scoped>
.laoban-m0 {
  display: flex;
  flex-direction: column;
  gap: 16px;
  min-height: 100%;
}

.laoban-header {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  gap: 12px 24px;

  h1 {
    margin-bottom: 4px;
  }
}

.laoban-config {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  gap: 2px 12px;
  margin: 0;
  font-size: 12px;
  max-width: 420px;

  dt {
    color: var(--muted);
  }

  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
}

.laoban-work {
  display: grid;
  grid-template-columns: minmax(280px, 2fr) minmax(0, 3fr);
  gap: 16px;
  flex: 1 1 auto;
  min-height: 0;

  @media (max-width: 960px) {
    grid-template-columns: minmax(0, 1fr);
  }
}

.laoban-panel {
  border: 1px solid var(--border);
  border-radius: var(--border-radius);
  background: var(--body-bg);
  padding: 16px;
  min-width: 0;

  h2 {
    margin: 0 0 12px;
  }
}

.laoban-chat form {
  display: flex;
  flex-direction: column;
  gap: 6px;

  textarea {
    resize: vertical;
    min-height: 100px;
  }

  .btn {
    align-self: flex-start;
    margin-top: 6px;
  }
}

.laoban-label {
  font-weight: bold;
}

.laoban-editing {
  margin-bottom: 12px;
  padding: 8px 12px;
  border-left: 3px solid var(--primary);
  background: var(--sortable-table-accent-bg, transparent);

  h4 {
    margin: 8px 0 4px;
  }
}

.laoban-history {
  list-style: none;
  padding: 0;
  margin: 0 0 8px;
  font-size: 12px;

  li {
    display: flex;
    flex-direction: column;
    margin-bottom: 4px;
  }
}

.laoban-hint {
  font-size: 12px;
  margin: 0;
}

.laoban-error {
  margin-top: 12px;
}

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

.laoban-text {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.laoban-view {
  display: flex;
  flex-direction: column;
  min-height: 460px;
}

.laoban-empty {
  margin: auto;
  text-align: center;
  padding: 24px;
}

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
</style>
