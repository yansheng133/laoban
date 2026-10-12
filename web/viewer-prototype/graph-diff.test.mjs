// Run with: node --test web/viewer-prototype/*.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { diffGraphs, edgesAtLevel, visibleAncestor, withPositions } from "./graph-diff.mjs";
import { SAMPLE_VERSIONS } from "./sample-versions.mjs";

const [v1, v2, v3] = SAMPLE_VERSIONS;
const status = (d, id) => d.nodes.find((n) => n.id === id)?.status;

test("identical versions have no changes", () => {
  const d = diffGraphs(v1, v1);
  assert.equal(d.summary.added + d.summary.removed + d.summary.changed + d.summary.moved, 0);
  assert.equal(d.summary.unchanged, v1.nodes.length + v1.edges.length);
});

test("added, removed and changed nodes are classified", () => {
  const d = diffGraphs(v2, v3);
  assert.equal(status(d, "mod-pay"), "added");
  assert.equal(status(d, "np-db"), "removed");
  assert.equal(status(d, "ing-web"), "changed");
  assert.equal(status(d, "app-web"), "unchanged");
  const ing = d.nodes.find((n) => n.id === "ing-web");
  assert.deepEqual(ing.changes, [{ field: "props.tls", before: false, after: true }]);
});

test("every node and edge of both versions appears exactly once", () => {
  const d = diffGraphs(v1, v3);
  const ids = new Set([...v1.nodes, ...v3.nodes].map((n) => n.id));
  assert.equal(d.nodes.length, ids.size);
  assert.equal(new Set(d.nodes.map((n) => n.id)).size, ids.size);
});

test("edge label change is a change, not remove plus add", () => {
  const d = diffGraphs(v2, v3);
  const e = d.edges.find((x) => x.edge.from === "app-web" && x.edge.to === "svc-api");
  assert.equal(e.status, "changed");
  assert.equal(e.before.label, "HTTP");
  assert.equal(e.edge.label, "HTTPS");
});

test("removed edges keep their endpoints", () => {
  const d = diffGraphs(v2, v3);
  const e = d.edges.find((x) => x.edge.from === "np-db");
  assert.equal(e.status, "removed");
});

test("position-only change is moved, not changed", () => {
  const moved = withPositions(v1, new Map([["app-web", { x: 0, z: 0 }]]), { id: "v1b", label: "v1b" });
  const d = diffGraphs(v1, moved);
  assert.equal(status(d, "app-web"), "moved");
  assert.equal(d.summary.changed, 0);
  assert.equal(d.summary.moved, 1);
});

test("withPositions does not mutate the source version", () => {
  const before = JSON.stringify(v1);
  const copy = withPositions(v1, new Map([["app-web", { x: 9, z: 9 }]]), { id: "x", label: "x" });
  copy.nodes[0].props.tls = "mutated";
  assert.equal(JSON.stringify(v1), before);
});

test("modules collapse onto their app at overview level", () => {
  const byId = new Map(v3.nodes.map((n) => [n.id, n]));
  assert.equal(visibleAncestor(byId, "mod-pay", 1), "app-api");
  assert.equal(visibleAncestor(byId, "mod-pay", 3), "mod-pay");
  const edges = edgesAtLevel(v3.nodes, v3.edges, 1);
  const lifted = edges.find((e) => e.from === "app-api" && e.to === "db-orders" && e.kind === "writes");
  assert.ok(lifted, "module->table writes should lift to app->database");
  assert.equal(lifted.lifted, true);
  assert.equal(lifted.sources.length, 2); // routes->orders and payment->payments
});

test("edges with an endpoint that has no visible ancestor are dropped", () => {
  // kitchen-worker and order-queue are detail 2 with no parent: hidden at level 1.
  const edges = edgesAtLevel(v2.nodes, v2.edges, 1);
  assert.ok(!edges.some((e) => e.to === "q-orders" || e.from === "app-kitchen"));
  assert.ok(edgesAtLevel(v2.nodes, v2.edges, 2).some((e) => e.from === "app-kitchen" && e.to === "q-orders"));
});

test("services lift onto their app so overview still shows who calls whom", () => {
  const edges = edgesAtLevel(v1.nodes, v1.edges, 1);
  assert.ok(edges.some((e) => e.from === "app-web" && e.to === "app-api" && e.kind === "calls"));
  assert.ok(edges.some((e) => e.from === "ing-web" && e.to === "app-web" && e.kind === "routes"));
});

test("parent cycles do not hang", () => {
  const nodes = [
    { id: "a", detail: 3, parent: "b" },
    { id: "b", detail: 3, parent: "a" },
  ];
  assert.equal(visibleAncestor(new Map(nodes.map((n) => [n.id, n])), "a", 1), null);
});
