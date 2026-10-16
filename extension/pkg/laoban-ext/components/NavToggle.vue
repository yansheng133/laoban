<script lang="ts">
// Hide or show Rancher's side menu on laoban's pages (utils/nav.ts).
import { defineComponent, onMounted, onBeforeUnmount } from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import {
  navCollapsed, setNavCollapsed, enterPage, leavePage
} from '../utils/nav';

export default defineComponent({
  name: 'LaobanNavToggle',

  setup() {
    const store = useStore();
    const { t } = useText(store);

    onMounted(enterPage);
    // The next laoban page applies it again; anything else gets the menu.
    onBeforeUnmount(leavePage);

    return {
      t, navCollapsed, toggle: () => setNavCollapsed(!navCollapsed.value)
    };
  },
});
</script>

<template>
  <button
    type="button"
    class="btn btn-sm role-link laoban-nav-toggle"
    :aria-pressed="navCollapsed ? 'true' : 'false'"
    data-testid="laoban-nav-toggle"
    @click="toggle"
  >
    <i
      class="icon"
      :class="navCollapsed ? 'icon-chevron-right' : 'icon-chevron-left'"
      aria-hidden="true"
    />
    {{ navCollapsed ? t('laoban.nav.showMenu') : t('laoban.nav.hideMenu') }}
  </button>
</template>

<style lang="scss" scoped>
.laoban-nav-toggle {
  padding-left: 0;
  min-height: 0;
  line-height: 1.5;
}
</style>
