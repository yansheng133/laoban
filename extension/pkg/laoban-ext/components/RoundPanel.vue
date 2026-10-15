<script lang="ts">
// 意見收斂 on the detail page: open a round, add opinions until the
// deadline, see the converged plan, confirm or object, and (initiator)
// skip people or cancel. Presentational: the page runs the requests
// (composables/useRound.ts) and passes the round in.
//
// Opinions and plans are user and AI text: rendered with {{ }} only.
import {
  defineComponent, ref, computed, onBeforeUnmount, PropType
} from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import { RoundView } from '../utils/backend';
import { OpinionDraft, MAX_BATCH } from '../utils/opinions';
import OpinionList from './OpinionList.vue';
import { userLabel } from '../utils/users';
import { formatDateTime, formatClock } from '../utils/time';

export const DEADLINES = [0, 10, 30, 60, 240, 1440];

export default defineComponent({
  name: 'LaobanRoundPanel',

  components: { OpinionList },

  props: {
    round: {
      type:    Object as PropType<RoundView | null>,
      default: null,
    },
    // Opinion being typed (the page also adds structure sentences to it).
    draft: {
      type:    String,
      default: '',
    },
    pending: {
      type:    String as PropType<string | null>,
      default: null,
    },
    elapsed: {
      type:    Number,
      default: 0,
    },
    error: {
      type:    String,
      default: '',
    },
    // Something else is running (a deploy, a direct save).
    disabled: {
      type:    Boolean,
      default: false,
    },
    // Opinions written for components in the 3D view, not sent yet.
    drafts: {
      type:    Array as PropType<OpinionDraft[]>,
      default: () => [],
    },
    // Node selected in the 3D view.
    selectedTarget: {
      type:    String as PropType<string | null>,
      default: null,
    },
    // A note from the page (the round disappeared, the deploy's progress).
    notice: {
      type:    String,
      default: '',
    },
  },

  emits: ['update:draft', 'open', 'act', 'add', 'remove-draft', 'support', 'reply', 'select-target'],

  setup(props, { emit }) {
    const store = useStore();
    const { t } = useText(store);
    const minutes = ref(0);
    const scope = ref('all');
    const objecting = ref(false);
    const objection = ref('');
    // Re-render the countdown, and the seconds the AI has been working.
    const now = ref(Date.now());

    const clock = setInterval(() => {
      now.value = Date.now();
    }, 1000);

    onBeforeUnmount(() => clearInterval(clock));

    // This page's confirm or skip is the last answer: the AI is now
    // changing the code. The backend answers at once and works in the
    // background; the page reads the round until it is done.
    const willApply = computed(() => {
      const r = props.round;

      if (!r || r.state !== 'confirming' || (props.pending !== 'confirm' && props.pending !== 'skip')) {
        return false;
      }

      return r.confirmers.filter((c) => c.answer === 'pending').length <= 1;
    });
    const state = computed(() => (willApply.value ? 'applying' : props.round?.state || 'none'));
    // Seconds since the round entered its current state (converging or
    // applying), from its log: the AI works in the background, so this is
    // the same for everyone looking at the page.
    const stateSeconds = computed(() => {
      const r = props.round;
      const entry = r ? [...r.log].reverse().find((e) => e.event === r.state) : null;
      const t0 = entry ? Date.parse(entry.at) : NaN;

      return Number.isNaN(t0) ? 0 : Math.max(0, Math.floor((now.value - t0) / 1000));
    });
    const active = computed(() => ['collecting', 'converging', 'confirming', 'applying', 'failed'].includes(state.value));
    const busy = computed(() => props.disabled || !!props.pending);
    const you = computed(() => props.round?.you);
    const who = (id: string) => (you.value?.user && id === you.value.user ? t('laoban.round.you') : userLabel(store, id));
    const deadlineText = computed(() => {
      const r = props.round;

      if (!r) {
        return '';
      }
      const d = new Date(r.deadline * 1000);
      const left = Math.max(0, Math.round((r.deadline * 1000 - now.value) / 60000));

      return t('laoban.round.deadline', { time: formatClock(store, d), minutes: left });
    });
    // The general opinion counts as one more.
    const count = computed(() => props.drafts.length + (props.draft.trim() ? 1 : 0));
    const tooMany = computed(() => count.value > MAX_BATCH);
    const canOpen = computed(() => !busy.value && !active.value && count.value > 0 && !tooMany.value);
    const canAdd = computed(() => !busy.value && !!you.value?.canAddOpinion && count.value > 0 && !tooMany.value);

    // Log entries in the page's language; the backend's Chinese text when
    // the code is unknown.
    const EVENTS = ['opened', 'opinion', 'objected', 'converging', 'converged', 'failed', 'confirmed', 'skipped', 'applying', 'done', 'cancelled', 'supported', 'unsupported', 'replied'];

    function logText(l: RoundView['log'][number]): string {
      if (!l.code || !EVENTS.includes(l.code)) {
        return l.event;
      }
      const args = { ...(l.args || {}) };

      if (args.user) {
        args.user = who(args.user);
      }

      return t(`laoban.round.events.${ l.code }`, args);
    }

    function formatTime(at: string): string {
      const d = new Date(at);

      return Number.isNaN(d.getTime()) ? at : formatDateTime(store, d);
    }

    function open() {
      if (canOpen.value) {
        emit('open', { minutes: minutes.value, scope: scope.value });
      }
    }

    function addOpinion() {
      if (canAdd.value) {
        emit('add');
      }
    }

    function sendObjection() {
      if (objection.value.trim() && !busy.value) {
        emit('act', 'object', { text: objection.value.trim() });
        objecting.value = false;
        objection.value = '';
      }
    }

    return {
      t,
      minutes,
      scope,
      deadlines: DEADLINES,
      objecting,
      objection,
      state,
      willApply,
      stateSeconds,
      active,
      busy,
      you,
      who,
      deadlineText,
      canOpen,
      canAdd,
      count,
      tooMany,
      maxBatch: MAX_BATCH,
      formatTime,
      logText,
      open,
      addOpinion,
      sendObjection,
    };
  },
});
</script>

<template>
  <div
    class="laoban-round"
    data-testid="laoban-round"
  >
    <p
      v-if="notice"
      class="laoban-round-notice"
      role="status"
    >
      {{ notice }}
    </p>
    <!-- No round running: open one. -->
    <form
      v-if="!active"
      class="laoban-form"
      @submit.prevent="open"
    >
      <p
        v-if="round && state === 'done'"
        class="text-muted laoban-hint"
      >
        {{ t('laoban.round.lastDone', { by: who(round.initiator) }) }}
      </p>
      <p
        v-else-if="round && state === 'cancelled'"
        class="text-muted laoban-hint"
      >
        {{ t('laoban.round.lastCancelled') }}
      </p>
      <div
        v-if="drafts.length"
        class="laoban-drafts"
      >
        <span class="laoban-label">{{ t('laoban.opinions.drafts', { count: drafts.length }) }}</span>
        <ul>
          <li
            v-for="d in drafts"
            :key="d.target"
          >
            <button
              type="button"
              class="laoban-opinion-target"
              @click="$emit('select-target', d.target)"
            >
              {{ d.targetName }}
            </button>
            <span class="laoban-text">{{ d.text }}</span>
            <button
              type="button"
              class="btn btn-sm role-link"
              :aria-label="t('laoban.opinions.removeDraft', { name: d.targetName })"
              :disabled="busy"
              @click="$emit('remove-draft', d.target)"
            >
              ✕
            </button>
          </li>
        </ul>
      </div>
      <p
        v-if="tooMany"
        class="text-error laoban-hint"
      >
        {{ t('laoban.opinions.tooMany', { max: maxBatch }) }}
      </p>
      <p
        v-else-if="!drafts.length"
        class="text-muted laoban-hint"
      >
        {{ t('laoban.opinions.howTo') }}
      </p>

      <details
        v-if="round && round.log.length"
        class="laoban-round-log"
      >
        <summary>{{ t('laoban.round.lastLog', { count: round.log.length }) }}</summary>
        <ul>
          <li
            v-for="(l, i) in round.log"
            :key="i"
          >
            <span class="text-muted">{{ formatTime(l.at) }}</span>
            {{ t('laoban.round.logLine', { who: l.by === 'laoban' ? 'laoban' : who(l.by), event: logText(l) }) }}
          </li>
        </ul>
      </details>
      <label
        for="laoban-opinion"
        class="laoban-label"
      >{{ t('laoban.round.opinionLabel') }}</label>
      <textarea
        id="laoban-opinion"
        :value="draft"
        rows="4"
        maxlength="1000"
        :placeholder="t('laoban.edit.promptPlaceholder')"
        :disabled="busy"
        @input="$emit('update:draft', $event.target.value)"
      />
      <div class="laoban-round-row">
        <label>
          {{ t('laoban.round.deadlineLabel') }}
          <select
            v-model.number="minutes"
            :disabled="busy"
          >
            <option
              v-for="m in deadlines"
              :key="m"
              :value="m"
            >
              {{ t(`laoban.round.deadlines.m${ m }`) }}
            </option>
          </select>
        </label>
        <label>
          {{ t('laoban.round.scopeLabel') }}
          <select
            v-model="scope"
            :disabled="busy"
          >
            <option
              v-for="s in ['all', 'web', 'api']"
              :key="s"
              :value="s"
            >
              {{ t(`laoban.round.scopes.${ s }`) }}
            </option>
          </select>
        </label>
      </div>
      <p class="text-muted laoban-hint">
        {{ minutes === 0 ? t('laoban.round.hintNow') : t('laoban.round.hintLater') }}
      </p>
      <button
        type="submit"
        class="btn role-primary"
        :disabled="!canOpen"
      >
        <i
          v-if="pending === 'open'"
          class="icon icon-spinner icon-spin"
          aria-hidden="true"
        />
        {{ minutes === 0 ? t('laoban.round.openNow') : t('laoban.round.open') }}<template v-if="count > 1">
          （{{ t('laoban.opinions.count', { count }) }}）
        </template>
      </button>
      <p
        v-if="pending === 'open' && minutes === 0"
        class="text-muted laoban-hint"
      >
        {{ t('laoban.round.converging', { seconds: elapsed }) }}
      </p>
    </form>

    <!-- A round is running. -->
    <template v-else-if="round">
      <div class="laoban-round-head">
        <strong>{{ t(`laoban.round.states.${ state }`) }}</strong>
        <span class="text-muted">
          {{ t('laoban.round.startedBy', { by: who(round.initiator) }) }} ·
          {{ t('laoban.round.scope', { scope: round.scope.map((s) => t(`laoban.round.scopes.${ s }`)).join('、') }) }}
        </span>
        <span
          v-if="state === 'collecting'"
          class="text-muted"
        >{{ deadlineText }}</span>
      </div>

      <OpinionList
        :opinions="round.opinions"
        :me="you ? you.user : ''"
        :can-discuss="!!you && !!you.canDiscuss"
        :busy="busy"
        :selected="selectedTarget"
        @support="$emit('support', $event)"
        @reply="(id, text) => $emit('reply', id, text)"
        @select="$emit('select-target', $event)"
      />

      <!-- Collecting: add an opinion. -->
      <form
        v-if="state === 'collecting'"
        class="laoban-form"
        @submit.prevent="addOpinion"
      >
        <div
          v-if="drafts.length"
          class="laoban-drafts"
        >
          <span class="laoban-label">{{ t('laoban.opinions.drafts', { count: drafts.length }) }}</span>
          <ul>
            <li
              v-for="d in drafts"
              :key="d.target"
            >
              <button
                type="button"
                class="laoban-opinion-target"
                @click="$emit('select-target', d.target)"
              >
                {{ d.targetName }}
              </button>
              <span class="laoban-text">{{ d.text }}</span>
              <button
                type="button"
                class="btn btn-sm role-link"
                :aria-label="t('laoban.opinions.removeDraft', { name: d.targetName })"
                :disabled="busy"
                @click="$emit('remove-draft', d.target)"
              >
                ✕
              </button>
            </li>
          </ul>
        </div>
        <p
          v-if="tooMany"
          class="text-error laoban-hint"
        >
          {{ t('laoban.opinions.tooMany', { max: maxBatch }) }}
        </p>
        <p
          v-else-if="!drafts.length"
          class="text-muted laoban-hint"
        >
          {{ t('laoban.opinions.howTo') }}
        </p>
        <label
          for="laoban-opinion"
          class="laoban-label"
        >{{ t('laoban.round.addLabel') }}</label>
        <textarea
          id="laoban-opinion"
          :value="draft"
          rows="3"
          maxlength="1000"
          :disabled="busy || !you || !you.canAddOpinion"
          @input="$emit('update:draft', $event.target.value)"
        />
        <button
          type="submit"
          class="btn role-primary"
          :disabled="!canAdd"
        >
          {{ count > 1 ? t('laoban.opinions.sendMany', { count }) : t('laoban.round.add') }}
        </button>
      </form>

      <p
        v-if="state === 'converging' || pending === 'converge'"
        class="text-muted"
      >
        <i
          class="icon icon-spinner icon-spin"
          aria-hidden="true"
        />
        {{ t('laoban.round.convergingCount', { count: round.opinions.length, seconds: pending === 'converge' ? elapsed : stateSeconds }) }}
      </p>

      <!-- Confirming: the plan and who still has to answer. -->
      <div
        v-if="state === 'confirming' && round.plan"
        class="laoban-round-plan"
      >
        <h3>{{ t('laoban.round.planTitle') }}</h3>
        <p class="laoban-text">
          {{ round.plan.summary }}
        </p>
        <ol>
          <li
            v-for="(s, i) in round.plan.steps"
            :key="i"
            class="laoban-text"
          >
            {{ s }}
          </li>
        </ol>
        <p
          v-if="round.plan.notes"
          class="laoban-text laoban-round-notes"
        >
          {{ t('laoban.round.notes') }}{{ round.plan.notes }}
        </p>
        <p class="text-muted laoban-hint">
          {{ t('laoban.round.planHint') }}
        </p>
        <ul class="laoban-round-confirmers">
          <li
            v-for="c in round.confirmers"
            :key="c.user"
          >
            <span>{{ who(c.user) }}</span>
            <span :class="c.answer === 'pending' ? 'text-muted' : 'text-success'">{{ t(`laoban.round.answers.${ c.answer }`) }}</span>
            <button
              v-if="c.answer === 'pending' && you && you.isInitiator && c.user !== you.user"
              type="button"
              class="btn btn-sm role-secondary"
              :disabled="busy"
              @click="$emit('act', 'skip', { user: c.user })"
            >
              {{ t('laoban.round.skip') }}
            </button>
          </li>
        </ul>
        <div
          v-if="you && you.mustConfirm"
          class="laoban-round-row"
        >
          <button
            type="button"
            class="btn role-primary"
            :disabled="busy"
            @click="$emit('act', 'confirm', {})"
          >
            <i
              v-if="pending === 'confirm'"
              class="icon icon-spinner icon-spin"
              aria-hidden="true"
            />
            {{ t('laoban.round.confirm') }}
          </button>
          <button
            type="button"
            class="btn role-secondary"
            :disabled="busy"
            @click="objecting = !objecting"
          >
            {{ t('laoban.round.object') }}
          </button>
        </div>
        <form
          v-if="objecting"
          class="laoban-form"
          @submit.prevent="sendObjection"
        >
          <textarea
            v-model="objection"
            rows="3"
            maxlength="1000"
            :placeholder="t('laoban.round.objectPlaceholder')"
            :aria-label="t('laoban.round.object')"
          />
          <button
            type="submit"
            class="btn role-secondary"
            :disabled="busy || !objection.trim()"
          >
            {{ t('laoban.round.objectSend') }}
          </button>
        </form>
      </div>

      <p
        v-if="state === 'applying'"
        class="text-muted"
        role="status"
      >
        <i
          class="icon icon-spinner icon-spin"
          aria-hidden="true"
        />
        {{ t('laoban.round.applying', { seconds: willApply ? elapsed : stateSeconds }) }}
      </p>

      <div
        v-if="state === 'failed'"
        class="laoban-round-row"
      >
        <p
          class="text-error"
          role="alert"
        >
          {{ round.message }}
        </p>
        <button
          v-if="you && you.isInitiator"
          type="button"
          class="btn role-primary"
          :disabled="busy"
          @click="$emit('act', 'converge', {})"
        >
          {{ t('laoban.round.retry') }}
        </button>
      </div>

      <button
        v-if="you && you.isInitiator && ['collecting', 'confirming', 'failed'].includes(state)"
        type="button"
        class="btn btn-sm role-secondary laoban-round-cancel"
        :disabled="busy"
        @click="$emit('act', 'cancel', {})"
      >
        {{ t('laoban.round.cancel') }}
      </button>

      <details class="laoban-round-log">
        <summary>{{ t('laoban.round.log', { count: round.log.length }) }}</summary>
        <ul>
          <li
            v-for="(l, i) in round.log"
            :key="i"
          >
            <span class="text-muted">{{ formatTime(l.at) }}</span>
            {{ t('laoban.round.logLine', { who: l.by === 'laoban' ? 'laoban' : who(l.by), event: logText(l) }) }}
          </li>
        </ul>
      </details>
    </template>

    <p
      v-if="error"
      class="laoban-error text-error"
      role="alert"
    >
      {{ error }}
    </p>
  </div>
</template>

<style lang="scss" scoped>
.laoban-round-notice {
  margin: 0;
  padding: 8px 10px;
  border-left: 3px solid var(--warning);
  background: var(--box-bg);
}

.laoban-round {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.laoban-round-head {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 10px;
  align-items: baseline;
  font-size: 13px;
}

.laoban-round-row {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  align-items: center;

  label {
    display: inline-flex;
    gap: 6px;
    align-items: center;
    font-size: 13px;
  }

  select {
    width: auto;
  }
}

.laoban-drafts {
  ul {
    list-style: none;
    padding: 0;
    margin: 4px 0 0;
  }

  li {
    display: flex;
    gap: 6px;
    align-items: baseline;
    padding: 2px 0;
    font-size: 13px;
  }
}

.laoban-opinion-target {
  background: none;
  border: none;
  padding: 0;
  color: var(--link);
  font-weight: bold;
  white-space: nowrap;
  cursor: pointer;
}

.laoban-round-opinions {
  padding-left: 20px;
  margin: 0;
  font-size: 13px;

  li {
    display: flex;
    flex-direction: column;
    margin-bottom: 6px;
  }
}

.laoban-round-plan {
  border-top: 1px solid var(--border);
  padding-top: 8px;

  h3 {
    margin: 0 0 4px;
  }

  ol {
    padding-left: 20px;
  }
}

.laoban-round-confirmers {
  list-style: none;
  padding: 0;
  margin: 6px 0;
  font-size: 13px;

  li {
    display: flex;
    gap: 10px;
    align-items: center;
    margin-bottom: 4px;
  }
}

.laoban-round-cancel {
  align-self: flex-start;
}

.laoban-round-log {
  font-size: 12px;

  summary {
    cursor: pointer;
  }

  ul {
    margin: 4px 0 0;
    padding-left: 16px;
  }
}
</style>
