<script lang="ts">
// The opinions of a round, grouped by the component they are about (the
// app in general first). Each opinion shows who +1'd it and its replies;
// people other than the author can +1 it, and anyone can reply, while the
// round collects opinions or waits for confirmation.
//
// Opinions and replies are user text: rendered with {{ }} only.
import {
  defineComponent, ref, computed, PropType
} from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import { RoundOpinion } from '../utils/backend';
import { userLabel } from '../utils/users';
import { formatDateTime } from '../utils/time';
import { groupOpinions } from '../utils/opinions';
import {
  DEFAULT_LIMITS, RoundLimits, chars, textState
} from '../utils/limits';
import CharCounter from './CharCounter.vue';

// An opinion longer than this is shown folded.
const LONG_CHARS = 280;
const LONG_LINES = 6;

export default defineComponent({
  name: 'LaobanOpinionList',

  components: { CharCounter },

  props: {
    opinions: {
      type:    Array as PropType<RoundOpinion[]>,
      default: () => [],
    },
    // The caller's user ID ("" when unknown).
    me: {
      type:    String,
      default: '',
    },
    // +1 and replies are possible now.
    canDiscuss: {
      type:    Boolean,
      default: false,
    },
    busy: {
      type:    Boolean,
      default: false,
    },
    // Node id selected in the 3D view: its group is highlighted.
    selected: {
      type:    String as PropType<string | null>,
      default: null,
    },
    limits: {
      type:    Object as PropType<RoundLimits>,
      default: () => ({ ...DEFAULT_LIMITS }),
    },
    // Characters the round still has room for (opinions and replies).
    roundLeft: {
      type:    Number,
      default: Number.POSITIVE_INFINITY,
    },
  },

  emits: ['support', 'reply', 'select'],

  setup(props, { emit }) {
    const store = useStore();
    const { t } = useText(store);
    // Opinion IDs whose replies are open, and the reply being typed.
    const open = ref<Record<string, boolean>>({});
    // Long opinions show their first lines until opened (report 20, N8).
    const full = ref<Record<string, boolean>>({});
    const isLong = (text: string) => chars(text) > LONG_CHARS || text.split('\n').length > LONG_LINES;

    function toggleFull(id: string) {
      full.value = { ...full.value, [id]: !full.value[id] };
    }
    const replyText = ref<Record<string, string>>({});

    const groups = computed(() => groupOpinions(props.opinions));
    const who = (id: string) => (props.me && id === props.me ? t('laoban.round.you') : userLabel(store, id));
    const supportedByMe = (o: RoundOpinion) => !!props.me && (o.supporters || []).includes(props.me);

    function toggle(id: string) {
      open.value = { ...open.value, [id]: !open.value[id] };
    }

    // Why a reply cannot be sent now ('' when it can).
    function replyProblem(o: RoundOpinion): string {
      const text = replyText.value[o.id] || '';

      if ((o.replies || []).length >= props.limits.replies) {
        return t('laoban.limits.repliesFull', { max: props.limits.replies });
      }
      if (props.roundLeft <= 0) {
        return t('laoban.limits.roundFull');
      }
      // Too long for the reply or for the room left: the counter says which.
      if (textState(text, props.limits.replyChars).over || chars(text) > props.roundLeft) {
        return 'tooLong';
      }

      return '';
    }

    function sendReply(id: string) {
      const text = (replyText.value[id] || '').trim();
      const o = props.opinions.find((x) => x.id === id);

      if (text && !props.busy && o && !replyProblem(o)) {
        emit('reply', id, text);
        replyText.value = { ...replyText.value, [id]: '' };
      }
    }

    // The number the AI's notes use for an opinion: its ID without the "o".
    const number = (id: string) => id.replace(/^o/, '');

    return {
      t, open, full, isLong, toggleFull, replyText, groups, who, supportedByMe, toggle, sendReply, replyProblem, number, formatTime: (at: string) => formatDateTime(store, at)
    };
  },
});
</script>

<template>
  <div class="laoban-opinions">
    <section
      v-for="g in groups"
      :key="g.key || '-'"
      class="laoban-opinion-group"
      :class="{ selected: !!g.key && g.key === selected }"
    >
      <h4>
        <button
          v-if="g.key"
          type="button"
          class="laoban-opinion-target"
          :title="t('laoban.opinions.showInView')"
          @click="$emit('select', g.key)"
        >
          {{ g.name }}
        </button>
        <span v-else>{{ t('laoban.opinions.general') }}</span>
        <span class="text-muted">（{{ g.opinions.length }}）</span>
      </h4>
      <ul>
        <li
          v-for="o in g.opinions"
          :key="o.id"
          class="laoban-opinion"
        >
          <div class="text-muted laoban-opinion-meta">
            <strong class="laoban-opinion-number">#{{ number(o.id) }}</strong> {{ who(o.by) }} · {{ formatTime(o.at) }}
          </div>
          <div
            class="laoban-text"
            :class="{ 'laoban-folded': isLong(o.text) && !full[o.id] }"
          >
            {{ o.text }}
          </div>
          <button
            v-if="isLong(o.text)"
            type="button"
            class="btn btn-sm role-link laoban-fold"
            :aria-expanded="full[o.id] ? 'true' : 'false'"
            @click="toggleFull(o.id)"
          >
            {{ full[o.id] ? t('laoban.opinions.showLess') : t('laoban.opinions.showMore') }}
          </button>
          <div class="laoban-opinion-actions">
            <button
              type="button"
              class="btn btn-sm"
              :class="supportedByMe(o) ? 'role-primary' : 'role-secondary'"
              :aria-pressed="supportedByMe(o) ? 'true' : 'false'"
              :disabled="busy || !canDiscuss || o.by === me || !me"
              :title="(o.supporters || []).map(who).join('、')"
              @click="$emit('support', o.id)"
            >
              +1 {{ (o.supporters || []).length || '' }}
            </button>
            <span
              v-if="(o.supporters || []).length"
              class="text-muted laoban-opinion-supporters"
            >{{ t('laoban.opinions.supportedBy', { names: (o.supporters || []).map(who).join('、') }) }}</span>
            <button
              type="button"
              class="btn btn-sm role-link"
              :aria-expanded="open[o.id] ? 'true' : 'false'"
              @click="toggle(o.id)"
            >
              {{ t('laoban.opinions.replies', { count: (o.replies || []).length }) }}
            </button>
          </div>
          <div
            v-if="open[o.id]"
            class="laoban-opinion-replies"
          >
            <p
              v-for="(r, i) in o.replies || []"
              :key="i"
            >
              <span class="text-muted">{{ who(r.by) }} · {{ formatTime(r.at) }}</span>
              <span class="laoban-text">{{ r.text }}</span>
            </p>
            <form
              v-if="canDiscuss"
              class="laoban-opinion-reply"
              @submit.prevent="sendReply(o.id)"
            >
              <input
                v-model="replyText[o.id]"
                type="text"
                :class="{ 'laoban-over': replyProblem(o) === 'tooLong' }"
                :aria-invalid="replyProblem(o) === 'tooLong' ? 'true' : 'false'"
                :placeholder="t('laoban.opinions.replyPlaceholder')"
                :aria-label="t('laoban.opinions.replyPlaceholder')"
                :disabled="busy || (o.replies || []).length >= limits.replies"
              >
              <button
                type="submit"
                class="btn btn-sm role-secondary"
                :disabled="busy || !(replyText[o.id] || '').trim() || !!replyProblem(o)"
              >
                {{ t('laoban.opinions.send') }}
              </button>
              <CharCounter
                v-if="replyText[o.id]"
                class="laoban-reply-counter"
                :text="replyText[o.id]"
                :max="limits.replyChars"
                :left="roundLeft"
              />
              <p
                v-if="replyProblem(o) && replyProblem(o) !== 'tooLong'"
                class="text-error laoban-reply-problem"
              >
                {{ replyProblem(o) }}
              </p>
            </form>
          </div>
        </li>
      </ul>
    </section>
  </div>
</template>

<style lang="scss" scoped>
.laoban-opinions {
  display: flex;
  flex-direction: column;
  gap: 8px;
  font-size: 13px;
}

.laoban-opinion-group {
  border-left: 3px solid var(--border);
  padding-left: 8px;

  &.selected {
    border-left-color: var(--primary);
  }

  h4 {
    margin: 0 0 4px;
    font-size: 13px;
  }

  ul {
    list-style: none;
    padding: 0;
    margin: 0;
  }
}

.laoban-opinion-target {
  background: none;
  border: none;
  padding: 0;
  color: var(--link);
  font-weight: bold;
  cursor: pointer;
}

.laoban-opinion {
  padding: 4px 0 6px;
  border-bottom: 1px solid var(--border);
}

.laoban-opinion-meta {
  font-size: 12px;
}

.laoban-opinion-number {
  color: var(--body-text);
}

.laoban-opinion-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 8px;
  align-items: center;
  margin-top: 4px;
}

.laoban-opinion-supporters {
  font-size: 12px;
}

.laoban-opinion-replies {
  margin: 4px 0 0 12px;

  p {
    display: flex;
    flex-direction: column;
    margin: 0 0 4px;
  }
}

// Wraps in a narrow column (the enlarged view's side) rather than
// pushing "Send" out of sight.
.laoban-opinion-reply {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;

  input {
    flex: 1 1 120px;
    min-width: 0;
  }

  button {
    flex: none;
  }
}
.laoban-over {
  border-color: var(--error) !important;
}

.laoban-reply-counter,
.laoban-reply-problem {
  flex-basis: 100%;
  margin: 0;
  font-size: 12px;
}
.laoban-folded {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 4;
  line-clamp: 4;
  overflow: hidden;
}

.laoban-fold {
  padding-left: 0;
}
</style>
