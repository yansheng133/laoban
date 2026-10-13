import Dashboard from '../pages/Dashboard.vue';
import Apps from '../pages/Apps.vue';
import AppDetail from '../pages/AppDetail.vue';
import {
  PRODUCT_NAME, BLANK_CLUSTER, DASHBOARD_PAGE, DASHBOARD_ROUTE_NAME, APPS_PAGE, APPS_ROUTE_NAME, APP_DETAIL_ROUTE_NAME
} from '../config/laoban';

const meta = {
  product: PRODUCT_NAME,
  cluster: BLANK_CLUSTER
};

const routes = [
  {
    name:      DASHBOARD_ROUTE_NAME,
    path:      `/${ PRODUCT_NAME }/c/:cluster/${ DASHBOARD_PAGE }`,
    component: Dashboard,
    meta,
  },
  {
    name:      APPS_ROUTE_NAME,
    path:      `/${ PRODUCT_NAME }/c/:cluster/${ APPS_PAGE }`,
    component: Apps,
    meta,
  },
  {
    name:      APP_DETAIL_ROUTE_NAME,
    path:      `/${ PRODUCT_NAME }/c/:cluster/${ APPS_PAGE }/:name`,
    component: AppDetail,
    meta,
  },
];

export default routes;
