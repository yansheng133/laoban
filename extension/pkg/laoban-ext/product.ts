import { IPlugin } from '@shell/core/types';
import {
  PRODUCT_NAME, BLANK_CLUSTER, DASHBOARD_PAGE, DASHBOARD_ROUTE_NAME
} from './config/laoban';

// Top-level product, following
// https://extensions.rancher.io/extensions/next/usecases/top-level-product
// The side-nav label comes from the `product.laoban` translation key.
export function init($plugin: IPlugin, store: any) {
  const { product, virtualType, basicType } = $plugin.DSL(store, PRODUCT_NAME);

  product({
    icon:    'gear',
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

  virtualType({
    labelKey: 'laoban.nav.dashboard',
    name:     DASHBOARD_PAGE,
    route:    {
      name:   DASHBOARD_ROUTE_NAME,
      params: {
        product: PRODUCT_NAME,
        cluster: BLANK_CLUSTER
      }
    }
  });

  basicType([DASHBOARD_PAGE]);
}
