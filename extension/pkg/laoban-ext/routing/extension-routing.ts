import Dashboard from '../pages/Dashboard.vue';
import {
  PRODUCT_NAME, BLANK_CLUSTER, DASHBOARD_PAGE, DASHBOARD_ROUTE_NAME
} from '../config/laoban';

const routes = [
  {
    name:      DASHBOARD_ROUTE_NAME,
    path:      `/${ PRODUCT_NAME }/c/:cluster/${ DASHBOARD_PAGE }`,
    component: Dashboard,
    meta:      {
      product: PRODUCT_NAME,
      cluster: BLANK_CLUSTER
    },
  },
];

export default routes;
