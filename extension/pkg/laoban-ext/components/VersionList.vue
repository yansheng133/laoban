<script lang="ts">
// The versions of one app (GET /api/apps/<name>/versions), newest first.
// "查看" selects a version for the 3D view (emit 'select'); "還原到這一版"
// asks for confirmation inline (no browser dialog) and then emits
// 'restore'. Requests come from users and summaries from AI: rendered with
// {{ }} only, never v-html.
import { defineComponent, ref, PropType } from 'vue';
import { useStore } from 'vuex';
import { useI18n } from '@shell/composables/useI18n';
import { safeHttpsUrl, VersionEntry } from '../utils/backend';

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

  setup(_props, { emit }) {
    const store = useStore();
    const { t } = useI18n(store);
    const confirming = ref<string | null>(null);

    function formatTime(at: string): string {
      const d = new Date(at);

      return Number.isNaN(d.getTime()) ? at : d.toLocaleString();
    }

    function kindLabel(kind: string): string {
      return ['create', 'edit', 'restore', 'manual', 'layout'].includes(kind) ? t(`laoban.versions.kind.${ kind }`) : t('laoban.versions.kind.commit');
    }

    function confirm(commit: string) {
      confirming.value = null;
      emit('restore', commit);
    }

    return {
      t, confirming, formatTime, kindLabel, confirm, safeHttpsUrl
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
        v-for="v in versions"
        :key="v.commit"
        :class="{ selected: selected === v.commit || (!selected && v.current) }"
      >
        <div class="laoban-version-head">
          <strong>{{ t('laoban.versions.number', { n: v.version }) }}</strong>
          <span
            v-if="v.current"
            class="badge-state bg-success"
          >{{ t('laoban.versions.current') }}</span>
          <span class="text-muted">{{ formatTime(v.at) }} · {{ kindLabel(v.kind) }}</span>
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
