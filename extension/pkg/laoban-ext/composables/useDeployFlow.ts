// Plan -> confirm -> deploy -> wait, shared by the "new app" page and the
// app detail page (where the same flow changes an existing app).
//
// After /api/apply the Fleet bundle is polled through Steve; once it is
// ready (or only "modified"), each Deployment from the apply response must
// run the committed pod template before the app link is offered
// (utils/rollout.ts).
import {
  ref, computed, onBeforeUnmount, Ref, ComputedRef
} from 'vue';
import {
  getConfig, createPlan, applyPlan, restoreVersion, listBundles, getDeployment, resolveAppUrl, safeHttpsUrl,
  aiRequestTimeoutMs, RequestError, ConfigResponse, PlanResponse, ApplyResponse, StoreLike
} from '../utils/backend';
import { bundleReadiness, Readiness } from '../utils/bundle';
import { deploymentRolledOut, rolloutTargets } from '../utils/rollout';
import { BUNDLE_POLL_INTERVAL_MS, BUNDLE_POLL_TIMEOUT_MS, FLEET_WORKSPACE } from '../config/backend';

export type Phase = 'idle' | 'planning' | 'planned' | 'applying' | 'deploying' | 'ready' | 'timeout';

type Translate = (key: string, args?: Record<string, unknown>) => string;

export interface DeployFlowHooks {
  // After a successful /api/apply (the commit exists).
  onApplied?: (applied: ApplyResponse) => void;
  // When the new version is running (or Fleet stopped at "modified").
  onReady?: (applied: ApplyResponse) => void;
}

export interface DeployFlow {
  config: Ref<ConfigResponse | null>;
  configError: Ref<string>;
  phase: Ref<Phase>;
  errorText: Ref<string>;
  plan: Ref<PlanResponse | null>;
  applied: Ref<ApplyResponse | null>;
  readiness: Ref<Readiness | null>;
  pollError: Ref<boolean>;
  rollingOut: Ref<boolean>;
  elapsed: Ref<number>;
  busy: ComputedRef<boolean>;
  canApply: ComputedRef<boolean>;
  steps: ComputedRef<string[]>;
  commitLink: ComputedRef<string | null>;
  shortCommit: ComputedRef<string>;
  appLink: ComputedRef<string | null>;
  statusText: ComputedRef<string>;
  showStatusMessage: ComputedRef<boolean>;
  loadConfig: () => Promise<void>;
  generatePlan: (prompt: string, opts?: { appName?: string; baseApp?: string }) => Promise<void>;
  confirmAndDeploy: () => Promise<void>;
  restore: (app: string, commit: string) => Promise<void>;
  // True while the current deploy is a restore (no AI involved).
  restoring: Ref<boolean>;
  reset: () => void;
  messageFor: (e: unknown) => string;
}

export function useDeployFlow(store: StoreLike, t: Translate, hooks: DeployFlowHooks = {}): DeployFlow {
  const config = ref<ConfigResponse | null>(null);
  const configError = ref('');
  const phase = ref<Phase>('idle');
  const errorText = ref('');
  const plan = ref<PlanResponse | null>(null);
  const applied = ref<ApplyResponse | null>(null);
  const readiness = ref<Readiness | null>(null);
  const pollError = ref(false);
  const rollingOut = ref(false);
  const elapsed = ref(0);
  const restoring = ref(false);

  let elapsedTimer: ReturnType<typeof setInterval> | null = null;
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let pollStarted = 0;
  let pollGeneration = 0;
  let unmounted = false;

  const busy = computed(() => phase.value === 'planning' || phase.value === 'applying');
  const canApply = computed(() => !busy.value && !!plan.value && !applied.value);
  const steps = computed(() => {
    const list = plan.value?.steps;

    return Array.isArray(list) ? list.map((s) => String(s)) : [];
  });
  const commitLink = computed(() => safeHttpsUrl(applied.value?.commitUrl));
  const shortCommit = computed(() => String(applied.value?.commit || '').slice(0, 12));
  const appLink = computed(() => resolveAppUrl(applied.value?.appUrl, window.location.origin));

  function messageFor(e: unknown): string {
    if (e instanceof RequestError && e.userMessage) {
      return e.userMessage;
    }
    if (e instanceof RequestError && e.code === 'timeout') {
      return t('laoban.dashboard.errors.timeout');
    }

    return t('laoban.dashboard.errors.generic');
  }

  function stopElapsed() {
    if (elapsedTimer) {
      clearInterval(elapsedTimer);
      elapsedTimer = null;
    }
  }

  function startElapsed() {
    stopElapsed();
    elapsed.value = 0;
    const start = Date.now();

    elapsedTimer = setInterval(() => {
      elapsed.value = Math.floor((Date.now() - start) / 1000);
    }, 1000);
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

  // True when every Deployment from the apply response runs the new template.
  async function rolledOut(gen: number): Promise<boolean | null> {
    const targets = rolloutTargets(applied.value?.rollout);

    try {
      for (const target of targets) {
        const d = await getDeployment(store, target.path);

        if (gen !== pollGeneration) {
          return null;
        }
        if (!deploymentRolledOut(d, target.contentHash)) {
          return false;
        }
      }
    } catch (e) {
      if (gen !== pollGeneration) {
        return null;
      }
      pollError.value = true;

      return false;
    }

    return true;
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
      const done = await rolledOut(gen);

      if (done === null) {
        return;
      }
      rollingOut.value = !done;
      if (done) {
        phase.value = 'ready';
        stopElapsed();
        if (applied.value) {
          hooks.onReady?.(applied.value);
        }

        return;
      }
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
    rollingOut.value = false;
    pollError.value = false;
    pollStarted = Date.now();
    phase.value = 'deploying';
    startElapsed();
    poll(pollGeneration);
  }

  function reset() {
    restoring.value = false;
    stopPolling();
    stopElapsed();
    errorText.value = '';
    plan.value = null;
    applied.value = null;
    readiness.value = null;
    rollingOut.value = false;
    pollError.value = false;
    phase.value = 'idle';
  }

  async function generatePlan(prompt: string, opts: { appName?: string; baseApp?: string } = {}) {
    if (busy.value || !prompt.trim()) {
      return;
    }
    reset();
    phase.value = 'planning';
    startElapsed();
    try {
      plan.value = await createPlan(store, prompt.trim(), opts.appName || undefined, aiRequestTimeoutMs(config.value), opts.baseApp);
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
    hooks.onApplied?.(applied.value);
    startPolling();
  }

  // An earlier version published again: same deploy and wait as an apply.
  async function restore(app: string, commit: string) {
    if (busy.value) {
      return;
    }
    reset();
    restoring.value = true;
    phase.value = 'applying';
    startElapsed();
    try {
      applied.value = await restoreVersion(store, app, commit, aiRequestTimeoutMs(config.value));
    } catch (e) {
      errorText.value = messageFor(e);
      phase.value = 'idle';
      restoring.value = false;
      stopElapsed();

      return;
    }
    hooks.onApplied?.(applied.value);
    startPolling();
  }

  const statusText = computed(() => {
    switch (phase.value) {
    case 'planning':
      return t('laoban.dashboard.status.planning', { seconds: elapsed.value });
    case 'planned':
      return t('laoban.dashboard.status.planned');
    case 'applying':
      return restoring.value ? t('laoban.versions.restoring', { seconds: elapsed.value }) : t('laoban.dashboard.status.applying', { seconds: elapsed.value });
    case 'deploying': {
      const r = readiness.value;

      if (!r || r.state === 'missing') {
        return t('laoban.dashboard.status.waitingBundle', { seconds: elapsed.value });
      }
      if (r.state === 'error') {
        return t('laoban.dashboard.status.bundleError', { ready: r.ready, desired: r.desired });
      }
      if (rollingOut.value) {
        return t('laoban.dashboard.status.rollingOut', { seconds: elapsed.value });
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

  // Fleet's own message is shown next to the status while deploying, and
  // after "ready" when Fleet stopped at "modified".
  const showStatusMessage = computed(() => !!readiness.value?.message &&
    (phase.value === 'deploying' || (phase.value === 'ready' && readiness.value?.state === 'modified')));

  onBeforeUnmount(() => {
    unmounted = true;
    stopPolling();
    stopElapsed();
  });

  return {
    config,
    configError,
    phase,
    errorText,
    plan,
    applied,
    readiness,
    pollError,
    rollingOut,
    elapsed,
    busy,
    canApply,
    steps,
    commitLink,
    shortCommit,
    appLink,
    statusText,
    showStatusMessage,
    loadConfig,
    generatePlan,
    confirmAndDeploy,
    restore,
    restoring,
    reset,
    messageFor,
  };
}
