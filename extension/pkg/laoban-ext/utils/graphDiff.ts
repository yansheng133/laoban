// Compare two architectures (two versions of an app, or the current app and
// a change plan) for the 3D view. Same rules as
// web/viewer-prototype/graph-diff.mjs: nodes match by id, edges by
// from/to/kind; a node whose only difference is its position is "moved",
// which is a layout change, not an architecture change.
//
// mergeForDiff() returns one architecture holding every node and edge of
// both sides (removed ones taken from the base) plus their status, so the 3D
// view can draw added, removed and changed parts in one scene. Pure
// functions, unit tested; inputs are untrusted and only read.
import type { Architecture, ArchNode, ArchEdge } from './layout';

export type DiffStatus = 'added' | 'removed' | 'changed' | 'moved' | 'unchanged';

export interface DiffSummary {
  added: number;
  removed: number;
  changed: number;
}

export interface MergedDiff {
  architecture: Architecture;
  nodes: Record<string, DiffStatus>;
  edges: Record<string, DiffStatus>;
  summary: DiffSummary;
}

const SEMANTIC_FIELDS: (keyof ArchNode)[] = ['layer', 'kind', 'name', 'detail', 'parent'];

// Removed nodes are drawn a little behind where they were, so they do not
// sit inside a new node that took the same place.
export const REMOVED_OFFSET_Z = 1.2;

export function edgeKey(e: { from?: unknown; to?: unknown; kind?: unknown }): string {
  return `${ String(e.from ?? '') }\u0000${ String(e.to ?? '') }\u0000${ String(e.kind ?? '') }`;
}

function nodes(a: Architecture | null | undefined): ArchNode[] {
  return Array.isArray(a?.nodes) ? a!.nodes!.filter((n) => n && typeof n.id === 'string') : [];
}

function edges(a: Architecture | null | undefined): ArchEdge[] {
  return Array.isArray(a?.edges) ? a!.edges!.filter((e) => e && typeof e === 'object') : [];
}

function propsDiffer(a: Record<string, unknown> = {}, b: Record<string, unknown> = {}): boolean {
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);

  for (const k of keys) {
    if (a[k] !== b[k]) {
      return true;
    }
  }

  return false;
}

function nodeStatus(before: ArchNode, after: ArchNode): DiffStatus {
  for (const f of SEMANTIC_FIELDS) {
    if ((before[f] ?? null) !== (after[f] ?? null)) {
      return 'changed';
    }
  }
  if (propsDiffer(before.props, after.props)) {
    return 'changed';
  }
  const bx = before.pos?.x;
  const bz = before.pos?.z;

  return bx === after.pos?.x && bz === after.pos?.z ? 'unchanged' : 'moved';
}

/** Everything in `base` and `target`, with what happened to it going from base to target. */
export function mergeForDiff(base: Architecture | null | undefined, target: Architecture | null | undefined): MergedDiff {
  const baseNodes = new Map(nodes(base).map((n) => [n.id, n]));
  const targetNodes = new Map(nodes(target).map((n) => [n.id, n]));
  const outNodes: ArchNode[] = [];
  const nodeState: Record<string, DiffStatus> = {};

  for (const n of targetNodes.values()) {
    const b = baseNodes.get(n.id);

    nodeState[n.id] = b ? nodeStatus(b, n) : 'added';
    outNodes.push(n);
  }
  for (const b of baseNodes.values()) {
    if (!targetNodes.has(b.id)) {
      nodeState[b.id] = 'removed';
      const pos = b.pos && typeof b.pos.x === 'number' && typeof b.pos.z === 'number' ? { x: b.pos.x, z: b.pos.z + REMOVED_OFFSET_Z } : b.pos;

      outNodes.push({ ...b, pos });
    }
  }

  const baseEdges = new Map(edges(base).map((e) => [edgeKey(e), e]));
  const targetEdges = new Map(edges(target).map((e) => [edgeKey(e), e]));
  const outEdges: ArchEdge[] = [];
  const edgeState: Record<string, DiffStatus> = {};

  for (const [k, e] of targetEdges) {
    const b = baseEdges.get(k);

    edgeState[k] = !b ? 'added' : (b.label ?? '') !== (e.label ?? '') ? 'changed' : 'unchanged';
    outEdges.push(e);
  }
  for (const [k, b] of baseEdges) {
    if (!targetEdges.has(k)) {
      edgeState[k] = 'removed';
      outEdges.push(b);
    }
  }

  const count = (s: DiffStatus) => Object.values(nodeState).filter((x) => x === s).length +
    Object.values(edgeState).filter((x) => x === s).length;

  return {
    architecture: { nodes: outNodes, edges: outEdges },
    nodes:        nodeState,
    edges:        edgeState,
    summary:      {
      added: count('added'), removed: count('removed'), changed: count('changed')
    },
  };
}

// One line of the change list next to the enlarged 3D view.
export interface DiffEntry {
  // Node id, or the edge key for a connection.
  id: string;
  type: 'node' | 'edge';
  status: 'added' | 'removed' | 'changed';
  // What to show: the component's name, or "from → to" for a connection.
  label: string;
  // Node kind or layer, for a small hint next to the label.
  kind: string;
  // For a connection: the node to select when it is clicked.
  nodeId: string;
}

const STATUS_ORDER: Record<DiffEntry['status'], number> = { added: 0, changed: 1, removed: 2 };

// The marked parts of a comparison as a list: added, then changed, then
// removed; components before connections; by name within each group.
export function diffEntries(d: MergedDiff | null | undefined): DiffEntry[] {
  if (!d) {
    return [];
  }
  const all = nodes(d.architecture);
  const byId = new Map(all.map((n) => [n.id, n]));
  const name = (id: string) => byId.get(id)?.name || id;
  const out: DiffEntry[] = [];

  for (const n of all) {
    const s = d.nodes[n.id];

    if (s === 'added' || s === 'removed' || s === 'changed') {
      out.push({
        id: n.id, type: 'node', status: s, label: name(n.id), kind: n.kind || n.layer, nodeId: n.id
      });
    }
  }
  for (const e of edges(d.architecture)) {
    const k = edgeKey(e);
    const s = d.edges[k];

    if (s === 'added' || s === 'removed' || s === 'changed') {
      out.push({
        id: k, type: 'edge', status: s, label: `${ name(String(e.from)) } → ${ name(String(e.to)) }`, kind: e.kind || '', nodeId: String(e.from)
      });
    }
  }

  return out.sort((a, b) => STATUS_ORDER[a.status] - STATUS_ORDER[b.status] ||
    (a.type === b.type ? 0 : a.type === 'node' ? -1 : 1) ||
    a.label.localeCompare(b.label));
}
