<script lang="ts">
import { defineComponent } from 'vue';
import { useStore } from 'vuex';
import { useI18n } from '@shell/composables/useI18n';

// Placeholder dashboard: no backend calls yet.
//
// Rule for this extension: every visible string goes through t(), and any
// text that may later come from AI output is rendered with text
// interpolation ({{ }}), never v-html. See extension/README.md.
const CARDS = [
  { key: 'requests', icon: 'icon-comment' },
  { key: 'architecture', icon: 'icon-cluster' },
  { key: 'review', icon: 'icon-checkmark' },
];

export default defineComponent({
  name: 'LaobanDashboard',

  setup() {
    const store = useStore();
    const { t } = useI18n(store);

    return { t, cards: CARDS };
  },
});
</script>

<template>
  <div class="laoban-dashboard">
    <header class="laoban-header">
      <h1>{{ t('laoban.dashboard.title') }}</h1>
      <p class="text-muted">
        {{ t('laoban.dashboard.description') }}
      </p>
    </header>

    <div class="laoban-cards">
      <section
        v-for="card in cards"
        :key="card.key"
        class="laoban-card"
        :data-testid="`laoban-card-${ card.key }`"
      >
        <h2>{{ t(`laoban.dashboard.cards.${ card.key }.title`) }}</h2>
        <div class="laoban-empty">
          <i
            class="icon icon-3x"
            :class="card.icon"
            aria-hidden="true"
          />
          <p class="laoban-empty-title">
            {{ t(`laoban.dashboard.cards.${ card.key }.emptyTitle`) }}
          </p>
          <p class="text-muted">
            {{ t(`laoban.dashboard.cards.${ card.key }.emptyDescription`) }}
          </p>
        </div>
      </section>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.laoban-header {
  margin-bottom: 20px;

  h1 {
    margin-bottom: 8px;
  }
}

.laoban-cards {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
  gap: 20px;
}

.laoban-card {
  border: 1px solid var(--border);
  border-radius: var(--border-radius);
  background: var(--body-bg);
  padding: 20px;

  h2 {
    margin: 0 0 16px;
  }
}

.laoban-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  text-align: center;
  padding: 24px 8px;

  .icon {
    color: var(--muted);
    margin-bottom: 12px;
  }
}

.laoban-empty-title {
  font-weight: bold;
  margin-bottom: 4px;
}
</style>
