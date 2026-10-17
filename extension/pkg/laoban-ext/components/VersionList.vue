<script lang="ts">
// The versions of one app (GET /api/apps/<name>/versions), newest first.
// "查看" selects a version for the 3D view (emit 'select'); "還原到這一版"
// asks for confirmation inline (no browser dialog) and then emits
// 'restore'. Requests come from users and summaries from AI: rendered with
// {{ }} only, never v-html.
import {
  defineComponent, ref, computed, PropType
} from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import { safeHttpsUrl, VersionEntry } from '../utils/backend';
import { userLabel } from '../utils/users';
import { currentUserId } from '../utils/me';
import { formatDateTime } from '../utils/time';
import { versionKindKey } from '../utils/versions';

const COLLAPSED = 5;

export default defineComponent({
  name: 'LaobanVersionList',

  props: {
    versions:  { type: Array as PropType<VersionEntry[]>, default: () => [] },
    selected:  { type: String as PropType<string | null>, default: null },
    busy:      { type: Boolean, default: false },
    loading:   { type: Boolean, default: false },
    error:     { type: String, default: '' },
    truncated: { type: Boolean, default: false },
    // Without the edit lock: versions can be viewed but not restored.
    readOnly:  { type: Boolean, default: false },
  },

  emits: ['select', 'restore'],

  setup(props, { emit }) {
    const store = useStore();
    const { t } = useText(store);
    const confirming = ref<string | null>(null);
    // The latest few, plus the selected one; the rest behind "show more".
    const showAll = ref(false);
    const shown = computed(() => (showAll.value ? props.versions : props.versions.filter((v, i) => i < COLLAPSED || v.commit === props.selected)));
    const hidden = computed(() => props.versions.length - shown.value.length);

    function formatTime(at: string): string {
      const d = new Date(at);

      return Number.isNaN(d.getTime()) ? at : formatDateTime(store, d);
    }

    function kindLabel(kind: string): string {
      return t(`laoban.versions.kind.${ versionKindKey(kind) }`);
    }

    function confirm(commit: string) {
      confirming.value = null;
      emit('restore', commit);
    }

    // The signed-in user's own versions read "you", as in past rounds
    // (report 23, N23).
    const me = ref('');

    currentUserId(store).then((id) => {
      me.value = id;
    });

    // Who made the version (verified by the API server), when recorded.
    function byLabel(v: VersionEntry): string {
      if (!v.by) {
        return '';
      }

      return me.value && v.by === me.value ? t('laoban.round.you') : userLabel(store, v.by);
    }

    return {
      t, confirming, formatTime, kindLabel, confirm, safeHttpsUrl, byLabel, showAll, shown, hidden
    };
  },
});
</script>

<template>
  <div
    class="laoban-versions"
    data-testid="laoban-versions"
  >
    <p
      v-if="loading && !versions.length"
      class="text-muted"
    >
      {{ t('laoban.detail.loading') }}
    </p>
    <p
      v-else-if="error"
      class="text-error"
    >
      {{ error }}
    </p>
    <ol class="laoban-version-list">
      <li
        v-for="v in shown"
        :key="v.commit"
        :class="{ selected: selected === v.commit || (!selected && v.current) }"
      >
        <div class="laoban-version-head">
          <strong>{{ t('laoban.versions.number', { n: v.version }) }}</strong>
          <span
            v-if="v.current"
            class="badge-state bg-success"
          >{{ t('laoban.versions.current') }}</span>
          <span class="text-muted">{{ formatTime(v.at) }} · {{ kindLabel(v.kind) }}<template v-if="byLabel(v)"> · {{ byLabel(v) }}</template><template v-if="v.model"> · {{ v.model }}</template></span>
          <a
            v-if="safeHttpsUrl(v.commitUrl)"
            :href="safeHttpsUrl(v.commitUrl)"
            target="_blank"
            rel="noopener noreferrer"
            class="laoban-version-commit"
          >{{ v.commit.slice(0, 7) }}</a>
        </div>
        <p
          v-if="v.request"
          class="laoban-text laoban-version-request"
        >
          {{ v.request }}
        </p>
        <div class="laoban-version-actions">
          <button
            type="button"
            class="btn btn-sm role-secondary"
            :disabled="busy || selected === v.commit || (!selected && v.current)"
            @click="$emit('select', v.current ? null : v.commit)"
          >
            {{ v.current ? t('laoban.versions.viewCurrent') : t('laoban.versions.view') }}
          </button>
          <template v-if="!v.current && !readOnly">
            <button
              v-if="confirming !== v.commit"
              type="button"
              class="btn btn-sm role-secondary"
              :disabled="busy"
              @click="confirming = v.commit"
            >
              {{ t('laoban.versions.restore') }}
            </button>
            <span
              v-else
              class="laoban-version-confirm"
              role="group"
            >
              <span>{{ t('laoban.versions.confirm', { n: v.version, next: versions.length ? versions[0].version + 1 : 0 }) }}</span>
              <button
                type="button"
                class="btn btn-sm role-primary"
                :disabled="busy"
                @click="confirm(v.commit)"
              >
                {{ t('laoban.versions.confirmYes') }}
              </button>
              <button
                type="button"
                class="btn btn-sm role-secondary"
                @click="confirming = null"
              >
                {{ t('laoban.versions.confirmNo') }}
              </button>
            </span>
          </template>
        </div>
      </li>
    </ol>
    <button
      v-if="hidden > 0 || showAll"
      type="button"
      class="btn btn-sm role-link laoban-version-more"
      @click="showAll = !showAll"
    >
      {{ showAll ? t('laoban.versions.showLess') : t('laoban.versions.showMore', { count: hidden }) }}
    </button>
    <p
      v-if="truncated"
      class="text-muted laoban-hint"
    >
      {{ t('laoban.versions.truncated') }}
    </p>
  </div>
</template>

<style lang="scss" scoped>
.laoban-version-list {
  list-style: none;
  padding: 0;
  margin: 0;

  li {
    padding: 8px;
    border-left: 3px solid transparent;
    border-bottom: 1px solid var(--border);

    &.selected {
      border-left-color: var(--primary);
      background: var(--sortable-table-hover-bg, transparent);
    }
  }
}

.laoban-version-head {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 8px;
  align-items: baseline;
  font-size: 13px;
}

.laoban-version-commit {
  font-family: monospace;
  font-size: 12px;
}

.laoban-version-request {
  margin: 4px 0;
  font-size: 13px;
}

.laoban-version-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
}

.laoban-version-confirm {
  display: inline-flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  font-size: 12px;
}

.laoban-text {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.laoban-hint {
  font-size: 12px;
  margin: 6px 0 0;
}
</style>
