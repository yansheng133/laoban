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

export default defineComponent({
  name: 'LaobanOpinionList',

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
  },

  emits: ['support', 'reply', 'select'],

  setup(props, { emit }) {
    const store = useStore();
    const { t } = useText(store);
    // Opinion IDs whose replies are open, and the reply being typed.
    const open = ref<Record<string, boolean>>({});
    const replyText = ref<Record<string, string>>({});

    const groups = computed(() => groupOpinions(props.opinions));
    const who = (id: string) => (props.me && id === props.me ? t('laoban.round.you') : userLabel(store, id));
    const supportedByMe = (o: RoundOpinion) => !!props.me && (o.supporters || []).includes(props.me);

    function toggle(id: string) {
      open.value = { ...open.value, [id]: !open.value[id] };
    }

    function sendReply(id: string) {
      const text = (replyText.value[id] || '').trim();

      if (text && !props.busy) {
        emit('reply', id, text);
        replyText.value = { ...replyText.value, [id]: '' };
      }
    }

    return {
      t, open, replyText, groups, who, supportedByMe, toggle, sendReply, formatTime: (at: string) => formatDateTime(store, at)
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
            {{ who(o.by) }} · {{ formatTime(o.at) }}
          </div>
          <div class="laoban-text">
            {{ o.text }}
          </div>
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
                maxlength="1000"
                :placeholder="t('laoban.opinions.replyPlaceholder')"
                :aria-label="t('laoban.opinions.replyPlaceholder')"
                :disabled="busy"
              >
              <button
                type="submit"
                class="btn btn-sm role-secondary"
                :disabled="busy || !(replyText[o.id] || '').trim()"
              >
                {{ t('laoban.opinions.send') }}
              </button>
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

.laoban-opinion-reply {
  display: flex;
  gap: 6px;

  input {
    flex: 1 1 auto;
    min-width: 0;
  }
}
</style>
