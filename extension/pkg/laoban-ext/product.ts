import { markRaw } from 'vue';
import { IPlugin } from '@shell/core/types';
import { ensureWaitingPoll, waitingToConfirm } from './utils/waiting';
import {
  PRODUCT_NAME, BLANK_CLUSTER, DASHBOARD_PAGE, DASHBOARD_ROUTE_NAME, APPS_PAGE, APPS_ROUTE_NAME
} from './config/laoban';

// Top-level product, following
// https://extensions.rancher.io/extensions/next/usecases/top-level-product
// The side-nav label comes from the `product.laoban` translation key.
export function init($plugin: IPlugin, store: any) {
  const {
    product, virtualType, basicType, configureType
  } = $plugin.DSL(store, PRODUCT_NAME);

  product({
    // The same three layers as the 3D view (assets/icon.svg).
    svg:     require('./assets/icon.svg'),
    inStore: 'management',
    weight:  100,
    to:      {
      name:   DASHBOARD_ROUTE_NAME,
      params: {
        product: PRODUCT_NAME,
        cluster: BLANK_CLUSTER
      }
    }
  });

  // Overview first in every language (Rancher sorts by label otherwise;
  // report 22, N15).
  virtualType({
    labelKey: 'laoban.nav.dashboard',
    name:     DASHBOARD_PAGE,
    weight:   2,
    route:    {
      name:   DASHBOARD_ROUTE_NAME,
      params: {
        product: PRODUCT_NAME,
        cluster: BLANK_CLUSTER
      }
    }
  });

  virtualType({
    labelKey: 'laoban.nav.apps',
    name:     APPS_PAGE,
    weight:   1,
    route:    {
      name:   APPS_ROUTE_NAME,
      params: {
        product: PRODUCT_NAME,
        cluster: BLANK_CLUSTER
      }
    }
  });

  basicType([DASHBOARD_PAGE, APPS_PAGE]);

  // The number of rounds waiting for the caller to confirm, next to
  // "Apps" (none shown when there are none).
  configureType(APPS_PAGE, {
    custom: {
      countGetter: markRaw(() => {
        ensureWaitingPoll(store);

        return waitingToConfirm.value || null;
      })
    }
  });
}
