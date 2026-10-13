// Shared names for the product, its pages and routes.
// Defined once so product.ts and routing stay in sync (as recommended in
// https://extensions.rancher.io/extensions/next/usecases/top-level-product).

export const PRODUCT_NAME = 'laoban';

// Rancher's "blank cluster" value for top-level products.
export const BLANK_CLUSTER = '_';

export const DASHBOARD_PAGE = 'dashboard';

export const DASHBOARD_ROUTE_NAME = `${ PRODUCT_NAME }-c-cluster-${ DASHBOARD_PAGE }`;

// App list, and one app (detail page where it is also changed).
export const APPS_PAGE = 'apps';

export const APPS_ROUTE_NAME = `${ PRODUCT_NAME }-c-cluster-${ APPS_PAGE }`;

export const APP_DETAIL_ROUTE_NAME = `${ PRODUCT_NAME }-c-cluster-${ APPS_PAGE }-name`;

/** Route location of an app's detail page. */
export function appDetailRoute(name: string) {
  return {
    name:   APP_DETAIL_ROUTE_NAME,
    params: {
      product: PRODUCT_NAME, cluster: BLANK_CLUSTER, name
    },
  };
}

// Locales that Rancher 2.15 / @rancher/shell 3.0.13 does not ship.
// Rancher ships only `en-us` and `zh-hans`; registering these makes them
// selectable in the language menu. Strings outside laoban fall back to English.
export const EXTRA_LOCALES: { locale: string; label: string }[] = [
  { locale: 'zh-hant', label: '繁體中文' },
  { locale: 'ja', label: '日本語' },
];
