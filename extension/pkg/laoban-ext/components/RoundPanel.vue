<script lang="ts">
// 意見收斂 on the detail page: open a round, add opinions until the
// deadline, see the converged plan, confirm or object, and (initiator)
// skip people or cancel. Presentational: the page runs the requests
// (composables/useRound.ts) and passes the round in.
//
// Opinions and plans are user and AI text: rendered with {{ }} only.
import {
  defineComponent, ref, computed, watch, onBeforeUnmount, PropType
} from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import { RoundView } from '../utils/backend';
import { OpinionDraft, RemovedDraft } from '../utils/opinions';
import {
  DEFAULT_LIMITS, RoundLimits, chars, roundSpace, textState
} from '../utils/limits';
import OpinionList from './OpinionList.vue';
import CharCounter from './CharCounter.vue';
import { userLabel } from '../utils/users';
import { formatDateTime, formatClock } from '../utils/time';

export const DEADLINES = [0, 10, 30, 60, 240, 1440];

export default defineComponent({
  name: 'LaobanRoundPanel',

  components: { OpinionList, CharCounter },

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
    // The last removed draft: "undo" brings it back.
    removedDraft: {
      type:    Object as PropType<RemovedDraft | null>,
      default: null,
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
    // Size limits, as the backend reports them.
    limits: {
      type:    Object as PropType<RoundLimits>,
      default: () => ({ ...DEFAULT_LIMITS }),
    },
    // False when the backend cannot store rounds (the Round CRD is missing).
    available: {
      type:    Boolean,
      default: true,
    },
    // Others may see this person's drafts (kept in the round).
    draftsShared: {
      type:    Boolean,
      default: true,
    },
  },

  emits: ['update:draft', 'update:draftsShared', 'open', 'act', 'add', 'remove-draft', 'undo-draft', 'support', 'reply', 'select-target',
    'edit', 'withdraw', 'deadline', 'comment', 'support-comment'],

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
      const entry = r ? [...r.log].reverse().find((e) => e.code === r.state) : null;
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
    const maxBatch = computed(() => props.limits.batch);
    const tooMany = computed(() => count.value > maxBatch.value);
    // Text that is too long stays as typed and cannot be sent.
    const draftState = computed(() => textState(props.draft, props.limits.opinionChars));
    const draftsTooLong = computed(() => props.drafts.some((d) => chars(d.text) > props.limits.opinionChars));
    const tooLong = computed(() => draftState.value.over || draftsTooLong.value);
    // How full the round is, with what is about to be sent. A new round
    // starts empty.
    const addingChars = computed(() => chars(props.draft) + props.drafts.reduce((n, d) => n + chars(d.text), 0));
    const space = computed(() => roundSpace(active.value ? props.round : null, props.limits, { chars: addingChars.value, opinions: count.value }));
    const objectionState = computed(() => textState(objection.value, props.limits.opinionChars));
    const objectionSpace = computed(() => roundSpace(props.round, props.limits, { chars: chars(objection.value), opinions: 1 }));
    const roundLeft = computed(() => roundSpace(props.round, props.limits, { chars: 0, opinions: 0 }).left);
    // Room for the general opinion: what the component drafts leave.
    const draftLeft = computed(() => Math.max(0, roundLeft.value - props.drafts.reduce((n, d) => n + chars(d.text), 0)));
    const canObject = computed(() => !busy.value && !!objection.value.trim() && !objectionState.value.over && !objectionSpace.value.over);
    const sendable = computed(() => count.value > 0 && !tooMany.value && !tooLong.value && !space.value.over);
    const canOpen = computed(() => !busy.value && props.available && !active.value && sendable.value);
    const canAdd = computed(() => !busy.value && props.available && !!you.value?.canAddOpinion && sendable.value);

    // Log entries in the page's language; the backend's Chinese text when
    // the code is unknown.
    const FAILURES = ['planTooLarge', 'planMissing', 'restartUnpublished', 'restartUnknown', 'mergeConflict', 'manualEditLost', 'aiInvalidOutput', 'aiUnavailable', 'budgetExceeded', 'spendLimit', 'githubFailed', 'versionConflict'];
    const EVENTS = ['opened', 'opinion', 'objected', 'converging', 'converged', 'failed', 'confirmed', 'skipped', 'applying', 'done', 'cancelled', 'supported', 'unsupported', 'replied', 'restarted',
      'edited', 'withdrawn', 'deadline', 'commented', 'emptyCancelled'];

    function logText(l: RoundView['log'][number]): string {
      if (!l.code || !EVENTS.includes(l.code)) {
        return l.event;
      }
      const args = { ...(l.args || {}) };

      // Opinions are numbered on the page as in the AI's notes: o3 is #3.
      if (args.opinion) {
        args.opinion = `#${ String(args.opinion).replace(/^o/, '') }`;
      }
      if (args.user) {
        args.user = who(args.user);
      }
      // A failure the page knows: in its language (report 20, N7).
      if (l.code === 'failed' && args.code && FAILURES.includes(args.code)) {
        args.message = t(`laoban.round.failures.${ args.code }`, args);
      }

      const code = l.code === 'deadline' && args.minutes === '0' ? 'deadlineNow' : l.code;

      return t(`laoban.round.events.${ code }`, args);
    }

    // Why the round failed, in the page's language when the backend gave a
    // known code; its Chinese text otherwise.
    const failureText = computed(() => {
      const r = props.round;

      if (r?.messageCode && FAILURES.includes(r.messageCode)) {
        return t(`laoban.round.failures.${ r.messageCode }`, r.messageArgs || {});
      }

      return r?.message || '';
    });

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

    // The initiator moves the deadline while collecting: only to choices
    // before the latest allowed, plus that latest time itself (report 21,
    // B14). Minutes from now.
    const minutesLeftToLatest = computed(() => {
      const latest = props.round?.latestDeadline;

      return latest ? Math.floor((latest - now.value / 1000) / 60) : 24 * 60;
    });
    const deadlineChoices = computed(() => {
      const max = minutesLeftToLatest.value;
      const list = DEADLINES.filter((m) => m > 0 && m <= max);

      return max > 0 && !list.includes(max) && max < 24 * 60 ? [...list, max] : list;
    });
    const newDeadline = ref(30);

    watch(deadlineChoices, (list) => {
      if (list.length && !list.includes(newDeadline.value)) {
        newDeadline.value = list[list.length - 1];
      }
    }, { immediate: true });

    // An error from these buttons is shown next to them, not at the end of
    // the panel where it would go unseen (report 21, B14).
    let deadlineAt = 0;
    const errorNearDeadline = computed(() => !!props.error && now.value - deadlineAt < 30 * 1000);

    function moveDeadline() {
      if (!busy.value) {
        deadlineAt = Date.now();
        emit('deadline', newDeadline.value);
      }
    }

    function convergeNow() {
      if (!busy.value) {
        deadlineAt = Date.now();
        emit('deadline', 0);
      }
    }

    // Comments on the plan's steps: which step's are open, the text typed.
    const stepOpen = ref<Record<number, boolean>>({});
    const stepText = ref<Record<number, string>>({});
    const commentsOf = (step: number) => (props.round?.planComments || []).filter((c) => c.step === step);
    const stepTooLong = (step: number) => textState(stepText.value[step] || '', props.limits.replyChars).over || chars(stepText.value[step] || '') > roundLeft.value;

    function toggleStep(step: number) {
      stepOpen.value = { ...stepOpen.value, [step]: !stepOpen.value[step] };
    }

    function sendComment(step: number) {
      const text = (stepText.value[step] || '').trim();

      if (text && !busy.value && !stepTooLong(step)) {
        emit('comment', step, text);
        stepText.value = { ...stepText.value, [step]: '' };
      }
    }

    // Drafts others share (read only).
    const sharedDrafts = computed(() => props.round?.drafts?.shared || []);

    function sendObjection() {
      if (canObject.value) {
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
      deadlineChoices,
      errorNearDeadline,
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
      maxBatch,
      draftState,
      draftsTooLong,
      space,
      objectionState,
      roundLeft,
      draftLeft,
      newDeadline,
      moveDeadline,
      convergeNow,
      stepOpen,
      stepText,
      commentsOf,
      stepTooLong,
      toggleStep,
      sendComment,
      sharedDrafts,
      objectionSpace,
      canObject,
      formatTime,
      logText,
      failureText,
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
    <p
      v-if="!available"
      class="text-error laoban-round-notice"
      role="alert"
    >
      {{ t('laoban.round.unavailable') }}
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
        v-if="removedDraft"
        class="laoban-hint laoban-undo"
        role="status"
      >
        {{ t('laoban.opinions.removed', { name: removedDraft.draft.targetName }) }}
        <button
          type="button"
          class="btn btn-sm role-link"
          :disabled="busy"
          @click="$emit('undo-draft')"
        >
          {{ t('laoban.opinions.undo') }}
        </button>
      </p>
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
      >{{ t('laoban.round.opinionLabel') }}</label>
      <textarea
        id="laoban-opinion"
        :value="draft"
        rows="4"
        :class="{ 'laoban-over': draftState.over }"
        :aria-invalid="draftState.over ? 'true' : 'false'"
        :placeholder="t('laoban.edit.promptPlaceholder')"
        :disabled="busy"
        @input="$emit('update:draft', $event.target.value)"
      />
      <CharCounter
        :text="draft"
        :max="limits.opinionChars"
      />
      <p
        v-if="draftsTooLong"
        class="text-error laoban-hint"
      >
        {{ t('laoban.limits.draftTooLong', { max: limits.opinionChars }) }}
      </p>
      <p
        v-if="space.over"
        class="text-error laoban-hint"
      >
        {{ t('laoban.limits.newRoundOver', { max: space.max }) }}
      </p>
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
      <div
        v-if="state === 'collecting' && you && you.isInitiator"
        class="laoban-round-row laoban-deadline"
      >
        <label>
          {{ t('laoban.round.moveDeadline') }}
          <select
            v-model.number="newDeadline"
            :disabled="busy"
          >
            <option
              v-for="m in deadlineChoices"
              :key="m"
              :value="m"
            >
              {{ deadlines.includes(m) ? t(`laoban.round.deadlines.m${ m }`) : t('laoban.round.deadlineLatest', { minutes: m }) }}
            </option>
          </select>
        </label>
        <button
          type="button"
          class="btn btn-sm role-secondary"
          :disabled="busy || !deadlineChoices.length"
          @click="moveDeadline"
        >
          {{ t('laoban.round.moveDeadlineApply') }}
        </button>
        <button
          type="button"
          class="btn btn-sm role-secondary"
          :disabled="busy || !round.opinions.some((o) => !o.withdrawn)"
          @click="convergeNow"
        >
          {{ t('laoban.round.convergeNow') }}
        </button>
        <p
          v-if="errorNearDeadline"
          class="text-error laoban-deadline-error"
          role="alert"
        >
          {{ error }}
        </p>
      </div>

      <OpinionList
        :opinions="round.opinions"
        :me="you ? you.user : ''"
        :can-discuss="!!you && !!you.canDiscuss"
        :busy="busy"
        :selected="selectedTarget"
        :limits="limits"
        :round-left="roundLeft"
        :can-edit="state === 'collecting' && !!you && !!you.canAddOpinion"
        @support="$emit('support', $event)"
        @reply="(id, text) => $emit('reply', id, text)"
        @edit="(id, text) => $emit('edit', id, text)"
        @withdraw="$emit('withdraw', $event)"
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
          v-if="removedDraft"
          class="laoban-hint laoban-undo"
          role="status"
        >
          {{ t('laoban.opinions.removed', { name: removedDraft.draft.targetName }) }}
          <button
            type="button"
            class="btn btn-sm role-link"
            :disabled="busy"
            @click="$emit('undo-draft')"
          >
            {{ t('laoban.opinions.undo') }}
          </button>
        </p>
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
          v-if="sharedDrafts.length"
          class="laoban-shared-drafts"
        >
          <summary>{{ t('laoban.opinions.othersDrafts', { count: sharedDrafts.length }) }}</summary>
          <ul>
            <li
              v-for="d in sharedDrafts"
              :key="d.user"
            >
              <strong>{{ who(d.user) }}</strong>
              <ul>
                <li
                  v-for="(it, i) in d.items"
                  :key="i"
                  class="laoban-text"
                >
                  <span
                    v-if="it.target"
                    class="laoban-opinion-target"
                  >{{ it.target.name }}</span>
                  {{ it.text }}
                </li>
              </ul>
            </li>
          </ul>
        </details>
        <label class="laoban-share-drafts">
          <input
            type="checkbox"
            :checked="draftsShared"
            @change="$emit('update:draftsShared', $event.target.checked)"
          >
          {{ t('laoban.opinions.shareDrafts') }}
        </label>
        <!-- Drafts are shared by default: say so once there is one. -->
        <p
          v-if="draftsShared && count > 0"
          class="text-muted laoban-hint laoban-shared-hint"
          data-testid="laoban-shared-hint"
        >
          {{ t('laoban.opinions.sharedHint') }}
        </p>
        <div
          v-if="space.shown"
          class="laoban-space"
          :class="{ warn: space.warn, over: space.over || space.full }"
          data-testid="laoban-space"
        >
          <div
            class="laoban-space-bar"
            role="meter"
            :aria-valuenow="space.used + space.adding"
            aria-valuemin="0"
            :aria-valuemax="space.max"
            :aria-label="t('laoban.limits.spaceLabel')"
          >
            <span :style="{ width: `${ Math.round(space.share * 100) }%` }" />
          </div>
          <p class="laoban-hint">
            <template v-if="space.full">
              {{ t('laoban.limits.roundFull') }}
            </template>
            <template v-else-if="space.over">
              {{ t('laoban.limits.roundOver', { left: space.left }) }}
            </template>
            <template v-else>
              {{ t('laoban.limits.roundSpace', { used: space.used, max: space.max, left: space.left }) }}
            </template>
          </p>
        </div>
        <label
          for="laoban-opinion"
          class="laoban-label"
        >{{ t('laoban.round.addLabel') }}</label>
        <textarea
          id="laoban-opinion"
          :value="draft"
          rows="3"
          :class="{ 'laoban-over': draftState.over || (space.over && !space.full) }"
          :aria-invalid="draftState.over || space.over ? 'true' : 'false'"
          :disabled="busy || !you || !you.canAddOpinion || space.full"
          @input="$emit('update:draft', $event.target.value)"
        />
        <CharCounter
          :text="draft"
          :max="limits.opinionChars"
          :left="draftLeft"
          :reserved="roundLeft - draftLeft"
          :quiet="space.full"
        />
        <p
          v-if="draftsTooLong"
          class="text-error laoban-hint"
        >
          {{ t('laoban.limits.draftTooLong', { max: limits.opinionChars }) }}
        </p>
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
        {{ t('laoban.round.convergingCount', { count: round.opinions.filter((o) => !o.withdrawn).length, seconds: pending === 'converge' ? elapsed : stateSeconds }) }}
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
        <ol class="laoban-plan-steps">
          <li
            v-for="(s, i) in round.plan.steps"
            :key="i"
          >
            <span class="laoban-text">{{ s }}</span>
            <button
              type="button"
              class="btn btn-sm role-link"
              :aria-expanded="stepOpen[i + 1] ? 'true' : 'false'"
              @click="toggleStep(i + 1)"
            >
              {{ t('laoban.round.stepComments', { count: commentsOf(i + 1).length }) }}
            </button>
            <div
              v-if="stepOpen[i + 1]"
              class="laoban-step-comments"
            >
              <p
                v-for="c in commentsOf(i + 1)"
                :key="c.id"
              >
                <span class="text-muted">{{ who(c.by) }}</span>
                <span class="laoban-text">{{ c.text }}</span>
                <!-- Not on one's own comment (report 21, B17); the count still shows. -->
                <button
                  v-if="!you || c.by !== you.user"
                  type="button"
                  class="btn btn-sm"
                  :class="you && c.supporters.includes(you.user) ? 'role-primary' : 'role-secondary'"
                  :disabled="busy || !you || !you.canComment"
                  @click="$emit('support-comment', c.id)"
                >
                  +1 {{ c.supporters.length || '' }}
                </button>
                <span
                  v-else-if="c.supporters.length"
                  class="text-muted"
                >+1 {{ c.supporters.length }}</span>
              </p>
              <form
                v-if="you && you.canComment"
                class="laoban-opinion-reply"
                @submit.prevent="sendComment(i + 1)"
              >
                <input
                  v-model="stepText[i + 1]"
                  type="text"
                  :class="{ 'laoban-over': stepTooLong(i + 1) }"
                  :placeholder="t('laoban.round.stepCommentPlaceholder')"
                  :aria-label="t('laoban.round.stepCommentPlaceholder')"
                  :disabled="busy"
                >
                <button
                  type="submit"
                  class="btn btn-sm role-secondary"
                  :disabled="busy || !(stepText[i + 1] || '').trim() || stepTooLong(i + 1)"
                >
                  {{ t('laoban.opinions.send') }}
                </button>
                <CharCounter
                  v-if="stepText[i + 1]"
                  :text="stepText[i + 1]"
                  :max="limits.replyChars"
                  :left="roundLeft"
                />
              </form>
            </div>
          </li>
        </ol>
        <p
          v-if="(round.planComments || []).length"
          class="text-muted laoban-hint"
        >
          {{ t('laoban.round.stepCommentsHint') }}
        </p>
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
            :class="{ 'laoban-over': objectionState.over }"
            :aria-invalid="objectionState.over ? 'true' : 'false'"
            :placeholder="t('laoban.round.objectPlaceholder')"
            :aria-label="t('laoban.round.object')"
          />
          <CharCounter
            :text="objection"
            :max="limits.opinionChars"
            :left="roundLeft"
          />
          <p
            v-if="objectionSpace.over && !objectionState.over"
            class="text-error laoban-hint"
          >
            {{ objectionSpace.full ? t('laoban.limits.roundFull') : t('laoban.limits.roundOver', { left: objectionSpace.left }) }}
          </p>
          <button
            type="submit"
            class="btn role-secondary"
            :disabled="!canObject"
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
          {{ failureText }}
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
      v-if="error && !errorNearDeadline"
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
.laoban-over {
  border-color: var(--error) !important;
  outline-color: var(--error);
}

.laoban-space {
  margin: 4px 0;

  .laoban-space-bar {
    height: 6px;
    border-radius: 3px;
    background: var(--border);
    overflow: hidden;

    span {
      display: block;
      height: 100%;
      background: var(--muted);
    }
  }

  &.warn .laoban-space-bar span {
    background: var(--warning);
  }

  &.over {
    color: var(--error);

    .laoban-space-bar span {
      background: var(--error);
    }
  }
}
.laoban-plan-steps > li {
  margin-bottom: 4px;

  > .btn {
    padding-left: 6px;
  }
}

.laoban-step-comments {
  margin: 4px 0 8px;
  padding-left: 8px;
  border-left: 2px solid var(--border);
  font-size: 13px;

  p {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 8px;
    align-items: baseline;
    margin: 0 0 4px;
  }
}

.laoban-deadline {
  font-size: 13px;
}

.laoban-deadline-error {
  flex-basis: 100%;
  margin: 4px 0 0;
}

.laoban-shared-drafts {
  font-size: 13px;

  ul {
    margin: 4px 0;
    padding-left: 16px;
  }
}

.laoban-share-drafts {
  display: flex;
  gap: 6px;
  align-items: center;
  font-size: 13px;
}
</style>
