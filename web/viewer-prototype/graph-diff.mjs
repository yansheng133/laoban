// Architecture graph model and diff for the laoban 3D viewer.
//
// A version is { id, label, nodes, edges }.
//   node: { id, layer: "network"|"code"|"data", kind, name, detail: 1..3,
//           parent?: node id, props?: {k: string|number|boolean}, pos: {x, z} }
//   edge: { from, to, kind, label? }
//
// Node identity is the id. A node whose only change is its position is
// "moved", which is a layout change, not an architecture change.

const SEMANTIC_FIELDS = ["layer", "kind", "name", "detail", "parent"];

function edgeKey(e) {
  return `${e.from}\u0000${e.to}\u0000${e.kind}`;
}

function propChanges(before = {}, after = {}) {
  const keys = [...new Set([...Object.keys(before), ...Object.keys(after)])].sort();
  const out = [];
  for (const k of keys) {
    const a = before[k];
    const b = after[k];
    if (a !== b) out.push({ field: `props.${k}`, before: a, after: b });
  }
  return out;
}

/** Field-level differences between two versions of the same node. */
export function nodeFieldChanges(before, after) {
  const out = [];
  for (const f of SEMANTIC_FIELDS) {
    if ((before[f] ?? null) !== (after[f] ?? null)) {
      out.push({ field: f, before: before[f], after: after[f] });
    }
  }
  return out.concat(propChanges(before.props, after.props));
}

function samePos(a, b) {
  return a.pos.x === b.pos.x && a.pos.z === b.pos.z;
}

/**
 * Diff two versions. Every node and edge of both versions appears exactly
 * once in the result, with a status of added, removed, changed, moved or
 * unchanged. Results are sorted by id (nodes) or key (edges) so the output
 * is stable.
 */
export function diffGraphs(base, target) {
  const baseNodes = new Map(base.nodes.map((n) => [n.id, n]));
  const targetNodes = new Map(target.nodes.map((n) => [n.id, n]));
  const nodes = [];
  for (const id of [...new Set([...baseNodes.keys(), ...targetNodes.keys()])].sort()) {
    const b = baseNodes.get(id);
    const t = targetNodes.get(id);
    if (!b) nodes.push({ id, status: "added", node: t, changes: [] });
    else if (!t) nodes.push({ id, status: "removed", node: b, changes: [] });
    else {
      const changes = nodeFieldChanges(b, t);
      const status = changes.length ? "changed" : samePos(b, t) ? "unchanged" : "moved";
      nodes.push({ id, status, node: t, before: b, changes });
    }
  }

  const baseEdges = new Map(base.edges.map((e) => [edgeKey(e), e]));
  const targetEdges = new Map(target.edges.map((e) => [edgeKey(e), e]));
  const edges = [];
  for (const key of [...new Set([...baseEdges.keys(), ...targetEdges.keys()])].sort()) {
    const b = baseEdges.get(key);
    const t = targetEdges.get(key);
    if (!b) edges.push({ key, status: "added", edge: t });
    else if (!t) edges.push({ key, status: "removed", edge: b });
    else if ((b.label ?? "") !== (t.label ?? "")) {
      edges.push({ key, status: "changed", edge: t, before: b });
    } else edges.push({ key, status: "unchanged", edge: t });
  }

  const count = (list, s) => list.filter((x) => x.status === s).length;
  const summary = {};
  for (const s of ["added", "removed", "changed", "moved", "unchanged"]) {
    summary[s] = count(nodes, s) + count(edges, s);
  }
  return { nodes, edges, summary };
}

/**
 * Nearest ancestor of `id` (including itself) visible at `level`, or null.
 * Used to collapse edges onto their visible parents at lower detail levels.
 */
export function visibleAncestor(byId, id, level) {
  const seen = new Set();
  let cur = byId.get(id);
  while (cur && !seen.has(cur.id)) {
    if (cur.detail <= level) return cur.id;
    seen.add(cur.id);
    cur = cur.parent ? byId.get(cur.parent) : undefined;
  }
  return null;
}

/**
 * Edges to draw at a detail level: endpoints are lifted to their nearest
 * visible ancestor, self-loops are dropped and duplicates merged.
 */
export function edgesAtLevel(nodes, edges, level) {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const out = new Map();
  for (const e of edges) {
    const from = visibleAncestor(byId, e.from, level);
    const to = visibleAncestor(byId, e.to, level);
    if (!from || !to || from === to) continue;
    const lifted = from !== e.from || to !== e.to;
    const key = `${from}\u0000${to}\u0000${e.kind}`;
    if (!out.has(key)) out.set(key, { ...e, from, to, lifted, sources: [e] });
    else out.get(key).sources.push(e);
  }
  return [...out.values()];
}

/** A new version with node positions replaced; everything else copied. */
export function withPositions(version, positions, meta) {
  return {
    ...meta,
    nodes: version.nodes.map((n) => ({
      ...n,
      props: { ...(n.props ?? {}) },
      pos: positions.get(n.id) ? { ...positions.get(n.id) } : { ...n.pos },
    })),
    edges: version.edges.map((e) => ({ ...e })),
  };
}
