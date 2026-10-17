<script lang="ts">
// 總覽: create a new app (docs/m0-design.md, "頁面（Rancher UI extension）").
// Left: requirement -> plan, right: 3D architecture, bottom: status bar.
// Existing apps are listed and changed on the 應用 pages (Apps.vue,
// AppDetail.vue); the plan/deploy flow is shared (composables/useDeployFlow).
//
// Rules for this page: every visible string goes through t(), and all text
// that comes from the backend or AI is rendered with {{ }} interpolation,
// never v-html. See extension/README.md.
import { defineComponent, ref, computed, onMounted } from 'vue';
import { useStore } from 'vuex';
import NavToggle from '../components/NavToggle.vue';
import { useText } from '../composables/useText';
import ArchitectureView from '../components/ArchitectureView.vue';
import PlanResult from '../components/PlanResult.vue';
import StatusBar from '../components/StatusBar.vue';
import { useDeployFlow } from '../composables/useDeployFlow';
import { APP_NAME_PATTERN } from '../config/backend';
import { canChangeApps } from '../utils/backend';
import {
  APPS_ROUTE_NAME, PRODUCT_NAME, BLANK_CLUSTER, appDetailRoute
} from '../config/laoban';

export default defineComponent({
  name: 'LaobanDashboard',

  components: {
    ArchitectureView, PlanResult, StatusBar, NavToggle
  },

  setup() {
    const store = useStore();
    const { t } = useText(store);
    const flow = useDeployFlow(store, t);

    const prompt = ref('');
    const appName = ref('');

    const appNameInvalid = computed(() => !!appName.value.trim() && !APP_NAME_PATTERN.test(appName.value.trim()));
    // A viewer may look but not make apps (report 21, N9).
    const mayChange = ref(true);

    onMounted(async() => {
      mayChange.value = await canChangeApps(store);
    });
    const canPlan = computed(() => mayChange.value && !flow.busy.value && !!prompt.value.trim() && !appNameInvalid.value);
    const appsRoute = { name: APPS_ROUTE_NAME, params: { product: PRODUCT_NAME, cluster: BLANK_CLUSTER } };
    const detailRoute = computed(() => (flow.applied.value?.appName ? appDetailRoute(flow.applied.value.appName) : null));

    function generatePlan() {
      if (canPlan.value) {
        flow.generatePlan(prompt.value, { appName: appName.value.trim() || undefined });
      }
    }

    const formatNumber = (n: unknown) => (typeof n === 'number' && Number.isFinite(n) ? n.toLocaleString() : '-');

    onMounted(flow.loadConfig);

    return {
      t,
      ...flow,
      prompt,
      appName,
      appNameInvalid,
      canPlan,
      mayChange,
      appsRoute,
      detailRoute,
      generatePlan,
      formatNumber,
    };
  },
});
</script>

<template>
  <div class="laoban-m0">
    <header class="laoban-header">
      <div>
        <NavToggle />
        <h1>{{ t('laoban.dashboard.title') }}</h1>
        <p class="text-muted">
          {{ t('laoban.dashboard.description') }}
        </p>
        <router-link :to="appsRoute">
          {{ t('laoban.dashboard.toApps') }}
        </router-link>
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

    <div class="laoban-work">
      <section
        class="laoban-panel"
        :aria-label="t('laoban.dashboard.chat.title')"
      >
        <h2>{{ t('laoban.dashboard.chat.title') }}</h2>
        <!-- Only when the backend answers: otherwise its own error says why. -->
        <p
          v-if="!mayChange && config"
          class="laoban-banner-info"
          role="status"
        >
          {{ t('laoban.access.viewOnly') }}
        </p>
        <!-- Only once the backend answers and the caller may make apps:
             without a backend there is nothing to send the request to
             (report 23, N22). -->
        <form
          v-if="config && mayChange"
          class="laoban-form"
          data-testid="laoban-plan-form"
          @submit.prevent="generatePlan"
        >
          <label
            for="laoban-prompt"
            class="laoban-label"
          >{{ t('laoban.dashboard.chat.promptLabel') }}</label>
          <textarea
            id="laoban-prompt"
            v-model="prompt"
            rows="5"
            maxlength="4000"
            :placeholder="t('laoban.dashboard.chat.promptPlaceholder')"
            :disabled="busy"
          />
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
            {{ t('laoban.dashboard.chat.generate') }}
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
        >
          <template #ready>
            <router-link
              v-if="detailRoute"
              :to="detailRoute"
              class="btn role-secondary"
            >
              {{ t('laoban.dashboard.result.viewApp') }}
            </router-link>
          </template>
        </PlanResult>
      </section>

      <section
        class="laoban-panel laoban-view"
        :aria-label="t('laoban.dashboard.arch.title')"
      >
        <h2>{{ t('laoban.dashboard.arch.title') }}</h2>
        <ArchitectureView
          v-if="plan"
          :architecture="plan.architecture"
        />
        <p
          v-else
          class="laoban-empty text-muted"
        >
          {{ t('laoban.dashboard.arch.empty') }}
        </p>
      </section>
    </div>

    <StatusBar
      :text="statusText"
      :spinning="busy || phase === 'deploying'"
      :message="showStatusMessage && readiness ? readiness.message : ''"
      :poll-error="phase === 'deploying' && pollError"
    />
  </div>
</template>

<style lang="scss" scoped>
@import '../styles/layout.scss';


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
</style>

<style lang="scss">
@import '../styles/global.scss';
</style>
