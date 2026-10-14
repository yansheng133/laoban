<script lang="ts">
// Structure changes from the 3D view (phase-1 "改結構": add a connection,
// remove a component, add a module). M0 does not change code by itself:
// each action adds a sentence to "要怎麼修改", and the normal plan ->
// confirm flow lets the AI change the code to match. Adding a database,
// a queue or an Ingress is not in M0's fixed templates, so those are shown
// disabled.
import {
  defineComponent, ref, computed, watch, PropType
} from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import { layoutArchitecture } from '../utils/layout';
import { moduleName, structureSentence, StructureAction } from '../utils/structure';

export default defineComponent({
  name: 'LaobanStructureEdit',

  props: {
    architecture: {
      type:    Object as PropType<unknown>,
      default: null,
    },
    selected: {
      type:    String as PropType<string | null>,
      default: null,
    },
    disabled: {
      type:    Boolean,
      default: false,
    },
  },

  emits: ['add', 'select'],

  setup(props, { emit }) {
    const store = useStore();
    const { t } = useText(store);

    const nodes = computed(() => layoutArchitecture(props.architecture).nodes);
    const modules = computed(() => nodes.value.filter((n) => n.kind === 'module'));
    const targets = computed(() => nodes.value.filter((n) => n.kind === 'module' || n.kind === 'app'));
    const selectedModule = computed(() => modules.value.find((n) => n.id === props.selected) || null);

    const renameTo = ref('');
    const addPart = ref<'web' | 'api'>('web');
    const addName = ref('');
    const addPurpose = ref('');
    const connectTo = ref('');

    watch(() => props.selected, () => {
      renameTo.value = '';
      connectTo.value = '';
    });

    const renameValid = computed(() => !!moduleName(renameTo.value) && moduleName(renameTo.value) !== selectedModule.value?.name);
    const addValid = computed(() => !!moduleName(addName.value));

    function push(action: StructureAction) {
      const s = structureSentence(action, t);

      if (s) {
        emit('add', s);
      }
    }

    function rename() {
      if (selectedModule.value && renameValid.value) {
        push({
          type: 'rename', from: selectedModule.value.name, to: moduleName(renameTo.value)
        });
        renameTo.value = '';
      }
    }

    function remove() {
      if (selectedModule.value) {
        push({ type: 'delete', name: selectedModule.value.name });
        emit('select', null);
      }
    }

    function connect() {
      const from = selectedModule.value;
      const to = targets.value.find((n) => n.id === connectTo.value);

      if (from && to && from.id !== to.id) {
        push({
          type: 'connect', from: from.name, to: to.name
        });
        connectTo.value = '';
      }
    }

    function add() {
      if (addValid.value) {
        push({
          type: 'add', part: addPart.value, name: moduleName(addName.value), purpose: addPurpose.value.trim().slice(0, 100)
        });
        addName.value = '';
        addPurpose.value = '';
      }
    }

    function onSelect(ev: Event) {
      const v = (ev.target as HTMLSelectElement).value;

      emit('select', v || null);
    }

    return {
      t,
      modules,
      targets,
      selectedModule,
      renameTo,
      renameValid,
      addPart,
      addName,
      addPurpose,
      addValid,
      connectTo,
      rename,
      remove,
      connect,
      add,
      onSelect,
    };
  },
});
</script>

<template>
  <details class="laoban-structure">
    <summary>{{ t('laoban.structure.title') }}</summary>
    <p class="text-muted laoban-hint">
      {{ t('laoban.structure.hint') }}
    </p>

    <div class="laoban-structure-row">
      <label for="laoban-structure-node">{{ t('laoban.structure.select') }}</label>
      <select
        id="laoban-structure-node"
        :value="selectedModule ? selectedModule.id : ''"
        :disabled="disabled"
        @change="onSelect"
      >
        <option value="">
          {{ t('laoban.structure.none') }}
        </option>
        <option
          v-for="m in modules"
          :key="m.id"
          :value="m.id"
        >
          {{ m.name }}
        </option>
      </select>
    </div>

    <template v-if="selectedModule">
      <div class="laoban-structure-row">
        <input
          v-model="renameTo"
          type="text"
          maxlength="40"
          :placeholder="t('laoban.structure.renameTo')"
          :aria-label="t('laoban.structure.renameTo')"
          :disabled="disabled"
        >
        <button
          type="button"
          class="btn btn-sm role-secondary"
          :disabled="disabled || !renameValid"
          @click="rename"
        >
          {{ t('laoban.structure.rename') }}
        </button>
      </div>
      <div class="laoban-structure-row">
        <select
          v-model="connectTo"
          :aria-label="t('laoban.structure.connectTo')"
          :disabled="disabled"
        >
          <option value="">
            {{ t('laoban.structure.connectTo') }}
          </option>
          <option
            v-for="n in targets.filter((x) => x.id !== selectedModule.id)"
            :key="n.id"
            :value="n.id"
          >
            {{ n.name }}
          </option>
        </select>
        <button
          type="button"
          class="btn btn-sm role-secondary"
          :disabled="disabled || !connectTo"
          @click="connect"
        >
          {{ t('laoban.structure.connect') }}
        </button>
      </div>
      <div class="laoban-structure-row">
        <button
          type="button"
          class="btn btn-sm role-secondary"
          :disabled="disabled"
          @click="remove"
        >
          {{ t('laoban.structure.delete') }}
        </button>
      </div>
    </template>

    <div class="laoban-structure-row">
      <select
        v-model="addPart"
        :aria-label="t('laoban.structure.addTo')"
        :disabled="disabled"
      >
        <option value="web">
          {{ t('laoban.structure.parts.web') }}
        </option>
        <option value="api">
          {{ t('laoban.structure.parts.api') }}
        </option>
      </select>
      <input
        v-model="addName"
        type="text"
        maxlength="40"
        :placeholder="t('laoban.structure.addName')"
        :aria-label="t('laoban.structure.addName')"
        :disabled="disabled"
      >
      <input
        v-model="addPurpose"
        type="text"
        maxlength="100"
        :placeholder="t('laoban.structure.addPurpose')"
        :aria-label="t('laoban.structure.addPurpose')"
        :disabled="disabled"
      >
      <button
        type="button"
        class="btn btn-sm role-secondary"
        :disabled="disabled || !addValid"
        @click="add"
      >
        {{ t('laoban.structure.add') }}
      </button>
    </div>

    <div class="laoban-structure-row">
      <button
        v-for="k in ['database', 'queue', 'ingress']"
        :key="k"
        type="button"
        class="btn btn-sm role-secondary"
        disabled
        :title="t('laoban.structure.unsupported')"
      >
        {{ t(`laoban.structure.addKind.${ k }`) }}（{{ t('laoban.structure.unsupported') }}）
      </button>
    </div>
  </details>
</template>

<style lang="scss" scoped>
.laoban-structure {
  margin-top: 8px;
  font-size: 13px;

  summary {
    cursor: pointer;
    font-weight: bold;
  }
}

.laoban-structure-row {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  margin-top: 6px;

  input,
  select {
    width: auto;
    flex: 1 1 140px;
    min-width: 0;
  }
}
</style>
