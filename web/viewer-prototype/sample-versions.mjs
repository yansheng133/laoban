// Synthetic example: a breakfast-shop ordering system, three versions.
// v3 removes the database NetworkPolicy on purpose, so the viewer can show
// a change the review policy treats as "exposes externally".

const v1 = {
  id: "v1",
  label: "v1",
  author: "local://u-biz",
  note: { "zh-TW": "第一版：點餐網頁與訂單 API", en: "First version: ordering page and order API", ja: "初版：注文ページと注文 API" },
  nodes: [
    { id: "ing-web", layer: "network", kind: "ingress", name: "order.example.test", detail: 1, props: { tls: false }, pos: { x: -6, z: 0 } },
    { id: "svc-web", layer: "network", kind: "service", name: "web", detail: 2, parent: "app-web", props: { type: "ClusterIP", port: 80 }, pos: { x: -2, z: -2 } },
    { id: "svc-api", layer: "network", kind: "service", name: "order-api", detail: 2, parent: "app-api", props: { type: "ClusterIP", port: 8080 }, pos: { x: 2, z: -2 } },
    { id: "np-db", layer: "network", kind: "netpol", name: "allow-api-to-db", detail: 3, parent: "db-orders", props: { from: "order-api" }, pos: { x: 6, z: 1 } },

    { id: "app-web", layer: "code", kind: "app", name: "web", detail: 1, props: { lang: "TypeScript", replicas: 1 }, pos: { x: -4, z: -1 } },
    { id: "app-api", layer: "code", kind: "app", name: "order-api", detail: 1, props: { lang: "Rust", replicas: 1 }, pos: { x: 2, z: -1 } },
    { id: "mod-routes", layer: "code", kind: "module", name: "routes", detail: 3, parent: "app-api", props: { file: "src/routes.rs" }, pos: { x: 0.5, z: 2.5 } },
    { id: "mod-menu", layer: "code", kind: "module", name: "menu", detail: 3, parent: "app-api", props: { file: "src/menu.rs" }, pos: { x: 3.5, z: 2.5 } },

    { id: "db-orders", layer: "data", kind: "database", name: "orders-db", detail: 1, props: { engine: "PostgreSQL", version: 16 }, pos: { x: 3, z: 0 } },
    { id: "tbl-orders", layer: "data", kind: "table", name: "orders", detail: 3, parent: "db-orders", props: { columns: 6 }, pos: { x: 1, z: 3 } },
    { id: "tbl-menu", layer: "data", kind: "table", name: "menu_items", detail: 3, parent: "db-orders", props: { columns: 5 }, pos: { x: 5, z: 3 } },
  ],
  edges: [
    { from: "ing-web", to: "svc-web", kind: "routes" },
    { from: "svc-web", to: "app-web", kind: "routes" },
    { from: "app-web", to: "svc-api", kind: "calls", label: "HTTP" },
    { from: "svc-api", to: "app-api", kind: "routes" },
    { from: "mod-routes", to: "tbl-orders", kind: "writes" },
    { from: "mod-menu", to: "tbl-menu", kind: "reads" },
    { from: "np-db", to: "db-orders", kind: "protects" },
  ],
};

function clone(v) {
  return JSON.parse(JSON.stringify(v));
}

const v2 = clone(v1);
Object.assign(v2, {
  id: "v2",
  label: "v2",
  author: "local://u-dev",
  note: { "zh-TW": "加上廚房工作程式與排隊佇列", en: "Add kitchen worker and queue", ja: "厨房ワーカーとキューを追加" },
});
v2.nodes.push(
  { id: "app-kitchen", layer: "code", kind: "app", name: "kitchen-worker", detail: 2, props: { lang: "Rust", replicas: 1 }, pos: { x: 6, z: -2 } },
  { id: "q-orders", layer: "data", kind: "queue", name: "order-queue", detail: 2, props: { engine: "Valkey" }, pos: { x: 7, z: -2 } },
);
v2.edges.push(
  { from: "app-api", to: "q-orders", kind: "writes", label: "enqueue" },
  { from: "app-kitchen", to: "q-orders", kind: "reads", label: "dequeue" },
);

const v3 = clone(v2);
Object.assign(v3, {
  id: "v3",
  label: "v3",
  author: "local://u-dev",
  note: { "zh-TW": "加上付款；開 TLS；拿掉資料庫的 NetworkPolicy", en: "Add payments; enable TLS; drop the database NetworkPolicy", ja: "決済を追加、TLS 有効化、DB の NetworkPolicy を削除" },
});
v3.nodes = v3.nodes.filter((n) => n.id !== "np-db");
v3.edges = v3.edges.filter((e) => e.from !== "np-db");
v3.nodes.find((n) => n.id === "ing-web").props.tls = true;
v3.nodes.find((n) => n.id === "app-api").props.replicas = 2;
v3.nodes.push(
  { id: "mod-pay", layer: "code", kind: "module", name: "payment", detail: 3, parent: "app-api", props: { file: "src/payment.rs" }, pos: { x: 2, z: 4.5 } },
  { id: "tbl-pay", layer: "data", kind: "table", name: "payments", detail: 3, parent: "db-orders", props: { columns: 7 }, pos: { x: 3, z: 5 } },
);
v3.edges.push({ from: "mod-pay", to: "tbl-pay", kind: "writes" });
v3.edges.find((e) => e.from === "app-web" && e.to === "svc-api").label = "HTTPS";

export const SAMPLE_VERSIONS = [v1, v2, v3];
