<script lang="ts">
// The app's four files in Rancher's code editor (@shell/components/CodeMirror).
// Saving is done by the page (POST /api/apps/<name>/files through the deploy
// flow), so it shows in the same status bar as an AI change; this component
// only loads the files, keeps the edits and emits `save`.
//
// The files came from AI output or another user: they are shown as editor
// text only, never rendered as HTML.
import {
  defineComponent, ref, computed, watch, onMounted, PropType
} from 'vue';
import { useStore } from 'vuex';
import { errorText } from '../utils/errors';
import { useText } from '../composables/useText';
import CodeMirror from '@shell/components/CodeMirror.vue';
import { _EDIT, _VIEW } from '@shell/config/query-params';
import {
  getFiles, AppFiles, AppFilesResponse
} from '../utils/backend';

interface FileEntry {
  key: string;
  label: string;
  part: 'web' | 'api';
  name: string;
  // Only yaml and javascript modes are loaded by the shell; others are plain text.
  mode: string;
}

export const FILES: FileEntry[] = [
  {
    key: 'web/index.html', label: 'index.html', part: 'web', name: 'index.html', mode: 'text/plain'
  },
  {
    key: 'web/app.js', label: 'app.js', part: 'web', name: 'app.js', mode: 'javascript'
  },
  {
    key: 'web/style.css', label: 'style.css', part: 'web', name: 'style.css', mode: 'text/plain'
  },
  {
    key: 'api/handler.py', label: 'handler.py', part: 'api', name: 'handler.py', mode: 'text/plain'
  },
];

function read(files: AppFiles | null, f: FileEntry): string {
  const part = files?.[f.part] as Record<string, unknown> | undefined;
  const v = part?.[f.name];

  return typeof v === 'string' ? v : '';
}

export default defineComponent({
  name: 'LaobanCodeEditor',

  components: { CodeMirror },

  props: {
    appName: {
      type:     String,
      required: true,
    },
    editable: {
      type:    Boolean,
      default: false,
    },
    busy: {
      type:    Boolean,
      default: false,
    },
    // Changed by the page after a new version is deployed: reload.
    reloadKey: {
      type:    Number,
      default: 0,
    },
    // Files a running round of 意見收斂 is about ("web/app.js", ...): shown
    // read-only until it ends.
    blockedFiles: {
      type:    Array as PropType<string[]>,
      default: () => [],
    },
    // The page shows this tab. CodeMirror measures its box when it is
    // created, so it is only created while visible (inside display:none it
    // would measure zero and draw a single line).
    visible: {
      type:    Boolean,
      default: true,
    },
  },

  emits: ['save', 'dirty', 'discard'],

  setup(props, { emit }) {
    const store = useStore();
    const { t } = useText(store);

    const base = ref<AppFilesResponse | null>(null);
    const draft = ref<Record<string, string>>({});
    const active = ref(FILES[0].key);
    const note = ref('');
    const loading = ref(false);
    const error = ref('');

    const changed = computed(() => FILES.filter((f) => base.value && draft.value[f.key] !== read(base.value.files, f)).map((f) => f.key));
    const canSave = computed(() => props.editable && !props.busy && !!base.value && changed.value.length > 0 &&
      !changed.value.some((k) => props.blockedFiles.includes(k)));

    watch(() => changed.value.length > 0, (dirty) => emit('dirty', dirty));

    async function load() {
      emit('discard');
      loading.value = true;
      try {
        const res = await getFiles(store, props.appName);

        base.value = res;
        draft.value = Object.fromEntries(FILES.map((f) => [f.key, read(res.files, f)]));
        note.value = '';
        error.value = '';
      } catch (e) {
        error.value = errorText(t, e, 'laoban.dashboard.errors.generic');
      } finally {
        loading.value = false;
      }
    }

    function onInput(key: string, value: string) {
      draft.value = { ...draft.value, [key]: value };
    }

    function discard() {
      if (base.value) {
        draft.value = Object.fromEntries(FILES.map((f) => [f.key, read(base.value!.files, f)]));
      }
      // The page clears a save error that no longer applies.
      emit('discard');
    }

    function save() {
      if (!canSave.value || !base.value) {
        return;
      }
      const d = draft.value;
      const files: AppFiles = {
        web: {
          'index.html': d['web/index.html'],
          'app.js':     d['web/app.js'],
          'style.css':  d['web/style.css'],
        },
        api: { 'handler.py': d['api/handler.py'] },
      };

      emit('save', {
        baseCommit: base.value.commit, files, note: note.value.trim()
      });
    }

    watch(() => [props.appName, props.reloadKey], load);
    onMounted(load);

    // With unsaved changes, Reload asks first: the first press turns the
    // button into "Discard changes and reload?" (report 21, N1).
    const reloadAsk = ref(false);

    watch(changed, () => {
      reloadAsk.value = false;
    });

    function reload() {
      if (changed.value.length && !reloadAsk.value) {
        reloadAsk.value = true;

        return;
      }
      reloadAsk.value = false;
      load();
    }

    return {
      t,
      files: FILES,
      base,
      draft,
      active,
      note,
      loading,
      error,
      changed,
      canSave,
      editMode: computed(() => (props.editable && !props.busy && !props.blockedFiles.includes(active.value) ? _EDIT : _VIEW)),
      activeFile: computed(() => FILES.find((f) => f.key === active.value) || null),
      onInput,
      reloadAsk,
      reload,
      discard,
      save,
      load,
    };
  },
});
</script>

<template>
  <div class="laoban-editor">
    <p
      v-if="blockedFiles.length"
      class="text-warning laoban-hint"
      role="status"
    >
      {{ t('laoban.editor.blocked', { files: blockedFiles.map((f) => f.split('/').pop()).join('、') }) }}
    </p>
    <p
      v-else
      class="text-muted laoban-hint"
    >
      {{ editable ? t('laoban.editor.hint') : t('laoban.editor.hintReadOnly') }}
    </p>
    <p
      v-if="loading && !base"
      class="text-muted"
    >
      <i
        class="icon icon-spinner icon-spin"
        aria-hidden="true"
      />
      {{ t('laoban.detail.loading') }}
    </p>
    <p
      v-if="error"
      class="text-error"
      role="alert"
    >
      {{ error }}
    </p>

    <template v-if="base">
      <div
        class="laoban-editor-tabs"
        role="tablist"
        :aria-label="t('laoban.editor.files')"
      >
        <button
          v-for="f in files"
          :key="f.key"
          type="button"
          role="tab"
          class="btn btn-sm"
          :class="active === f.key ? 'role-primary' : 'role-secondary'"
          :aria-selected="active === f.key"
          @click="active = f.key"
        >
          {{ f.label }}<span
            v-if="changed.includes(f.key)"
            class="laoban-editor-dot"
            :title="t('laoban.editor.changedFile')"
          > ●</span>
        </button>
      </div>

      <div
        v-if="visible && activeFile"
        class="laoban-editor-cm"
        role="tabpanel"
      >
        <!-- One editor for the active file, recreated on switch; the edits
             live in `draft`, so nothing is lost. -->
        <CodeMirror
          :key="`${ activeFile.key }-${ base.commit }`"
          :value="draft[activeFile.key]"
          :mode="editMode"
          :options="{ mode: activeFile.mode, foldGutter: false, lineWrapping: false }"
          @onInput="onInput(activeFile.key, $event)"
        />
      </div>

      <form
        v-if="editable"
        class="laoban-editor-save"
        @submit.prevent="save"
      >
        <label
          for="laoban-editor-note"
          class="laoban-label"
        >{{ t('laoban.editor.note') }}</label>
        <input
          id="laoban-editor-note"
          v-model="note"
          type="text"
          maxlength="200"
          :placeholder="t('laoban.editor.notePlaceholder')"
          :disabled="busy"
        >
        <div class="laoban-editor-actions">
          <span class="text-muted">{{ changed.length ? t('laoban.editor.changed', { count: changed.length }) : t('laoban.editor.unchanged') }}</span>
          <button
            type="button"
            class="btn role-secondary"
            :disabled="busy || !changed.length"
            @click="discard"
          >
            {{ t('laoban.editor.discard') }}
          </button>
          <button
            type="button"
            class="btn"
            :class="reloadAsk ? 'role-primary bg-error' : 'role-secondary'"
            :disabled="busy || loading"
            @click="reload"
            @blur="reloadAsk = false"
          >
            {{ reloadAsk ? t('laoban.editor.reloadConfirm') : t('laoban.editor.reload') }}
          </button>
          <button
            type="submit"
            class="btn role-primary"
            :disabled="!canSave"
          >
            {{ t('laoban.editor.save') }}
          </button>
        </div>
      </form>
    </template>
  </div>
</template>

<style lang="scss" scoped>
.laoban-editor {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.laoban-editor-tabs {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.laoban-editor-dot {
  font-size: 10px;
}

.laoban-editor-cm {
  border: 1px solid var(--border);
  border-radius: var(--border-radius);
  min-height: 320px;
  max-height: 60vh;
  overflow: auto;

  :deep(.CodeMirror) {
    height: auto;
    min-height: 320px;
    font-size: 13px;
  }
}

.laoban-editor-save {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.laoban-editor-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  align-items: center;

  span {
    margin-right: auto;
    font-size: 12px;
  }
}
</style>
