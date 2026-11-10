<script lang="ts">
// Deleting an app: asks for the app's name first (inline, no browser
// dialog), then DELETE .../apps/<name> (one commit removing apps/<name>/),
// then waits until Fleet has removed the app's Deployments from the
// cluster. Emits `deleted` with whether the cluster was cleared in time,
// or `cancel`; `busy` while it deletes and waits, so the page around it can
// keep people from closing it or changing the app in the meantime.
import { defineComponent, ref, computed, onBeforeUnmount } from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import { errorText } from '../utils/errors';
import { deleteApp, getDeployment } from '../utils/backend';
import { deploymentTargets, allGone } from '../utils/rollout';

// How long the page waits for Fleet, and how often it looks.
export const CLEAR_TIMEOUT_MS = 3 * 60 * 1000;
const CLEAR_POLL_MS = 3000;

export default defineComponent({
  name: 'LaobanDeleteApp',

  props: {
    appName:   { type: String, required: true },
    // Why deleting is not possible right now (shown instead of the form).
    blockedBy: { type: String, default: '' },
  },

  emits: ['cancel', 'deleted', 'busy'],

  setup(props, { emit }) {
    const store = useStore();
    const { t } = useText(store);

    const typed = ref('');
    const phase = ref<'confirm' | 'deleting' | 'clearing'>('confirm');
    const error = ref('');
    const matches = computed(() => typed.value.trim() === props.appName);
    // Left the page: stop waiting (an object, so the loop sees the change).
    const life = { stopped: false };

    onBeforeUnmount(() => {
      life.stopped = true;
    });

    async function waitUntilCleared(paths: string[]): Promise<boolean> {
      const until = Date.now() + CLEAR_TIMEOUT_MS;

      while (!life.stopped && Date.now() < until) {
        try {
          if (allGone(await Promise.all(paths.map((p) => getDeployment(store, p))))) {
            return true;
          }
        } catch {
          // Reading Deployments is a nice-to-have: keep waiting.
        }
        await new Promise((resolve) => setTimeout(resolve, CLEAR_POLL_MS));
      }

      return false;
    }

    async function submit() {
      if (!matches.value || phase.value !== 'confirm' || props.blockedBy) {
        return;
      }
      error.value = '';
      phase.value = 'deleting';
      emit('busy', true);
      let paths: string[];

      try {
        const res = await deleteApp(store, props.appName);

        paths = deploymentTargets(res.deployments).map((d) => d.path);
      } catch (e) {
        phase.value = 'confirm';
        emit('busy', false);
        error.value = errorText(t, e, 'laoban.delete.failed');

        return;
      }
      phase.value = 'clearing';
      const cleared = paths.length ? await waitUntilCleared(paths) : false;

      emit('busy', false);
      if (!life.stopped) {
        emit('deleted', { appName: props.appName, cleared });
      }
    }

    return {
      t, typed, phase, error, matches, submit
    };
  },
});
</script>

<template>
  <div
    class="laoban-delete laoban-banner-error"
    role="group"
    :aria-label="t('laoban.delete.title', { name: appName })"
    data-testid="laoban-delete"
  >
    <h3>{{ t('laoban.delete.title', { name: appName }) }}</h3>
    <p
      v-if="blockedBy"
      role="status"
    >
      {{ blockedBy }}
    </p>
    <template v-else-if="phase === 'confirm'">
      <ul class="laoban-delete-effects">
        <li>{{ t('laoban.delete.effectRepo', { path: `apps/${ appName }` }) }}</li>
        <li>{{ t('laoban.delete.effectCluster') }}</li>
        <li>{{ t('laoban.delete.effectHistory') }}</li>
      </ul>
      <form
        class="laoban-delete-form"
        @submit.prevent="submit"
      >
        <label :for="`laoban-delete-${ appName }`">{{ t('laoban.delete.typeName', { name: appName }) }}</label>
        <input
          :id="`laoban-delete-${ appName }`"
          v-model="typed"
          type="text"
          autocomplete="off"
          spellcheck="false"
          data-testid="laoban-delete-input"
        >
        <div class="laoban-delete-buttons">
          <button
            type="button"
            class="btn btn-sm role-secondary"
            @click="$emit('cancel')"
          >
            {{ t('laoban.delete.cancel') }}
          </button>
          <button
            type="submit"
            class="btn btn-sm bg-error"
            :disabled="!matches"
            data-testid="laoban-delete-submit"
          >
            {{ t('laoban.delete.submit') }}
          </button>
        </div>
      </form>
      <p
        v-if="error"
        class="text-error"
        role="alert"
      >
        {{ error }}
      </p>
    </template>
    <p
      v-else
      role="status"
    >
      <i
        class="icon icon-spinner icon-spin"
        aria-hidden="true"
      />
      {{ phase === 'deleting' ? t('laoban.delete.deleting') : t('laoban.delete.clearing') }}
    </p>
    <div
      v-if="blockedBy"
      class="laoban-delete-buttons"
    >
      <button
        type="button"
        class="btn btn-sm role-secondary"
        @click="$emit('cancel')"
      >
        {{ t('laoban.delete.close') }}
      </button>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.laoban-delete {
  margin: 8px 0;
  padding: 12px 16px;
  white-space: normal;
  text-align: left;

  h3 {
    margin: 0 0 8px;
    font-size: 15px;
  }

  p {
    margin: 4px 0;
  }
}

.laoban-delete-effects {
  margin: 0 0 8px;
  padding-left: 20px;
}

.laoban-delete-form {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-width: 420px;

  input {
    font-family: monospace;
  }
}

.laoban-delete-buttons {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 4px;
}
</style>
