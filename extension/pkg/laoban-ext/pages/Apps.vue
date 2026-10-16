<script lang="ts">
// 應用: every app laoban has made, newest change first. Names lead to the
// app's detail page, where it can be changed; "新增應用" goes to 總覽.
import { defineComponent, ref, onMounted } from 'vue';
import { useStore } from 'vuex';
import NavToggle from '../components/NavToggle.vue';
import { useText } from '../composables/useText';
import AppList from '../components/AppList.vue';
import { canChangeApps } from '../utils/backend';
import { DASHBOARD_ROUTE_NAME, PRODUCT_NAME, BLANK_CLUSTER } from '../config/laoban';

export default defineComponent({
  name: 'LaobanApps',

  components: { AppList, NavToggle },

  setup() {
    const store = useStore();
    const { t } = useText(store);
    const createRoute = { name: DASHBOARD_ROUTE_NAME, params: { product: PRODUCT_NAME, cluster: BLANK_CLUSTER } };
    // A viewer sees no "New app" (report 21, N9).
    const mayChange = ref(true);

    onMounted(async() => {
      mayChange.value = await canChangeApps(store);
    });

    return { t, createRoute, mayChange };
  },
});
</script>

<template>
  <div class="laoban-m0">
    <header class="laoban-header">
      <div>
        <NavToggle />
        <h1>{{ t('laoban.apps.title') }}</h1>
        <p class="text-muted">
          {{ t('laoban.apps.description') }}
        </p>
      </div>
      <router-link
        v-if="mayChange"
        :to="createRoute"
        class="btn role-primary"
      >
        {{ t('laoban.apps.create') }}
      </router-link>
    </header>

    <div class="laoban-panel">
      <AppList />
    </div>
  </div>
</template>

<style lang="scss" scoped>
@import '../styles/layout.scss';

</style>

<style lang="scss">
@import '../styles/global.scss';
</style>
