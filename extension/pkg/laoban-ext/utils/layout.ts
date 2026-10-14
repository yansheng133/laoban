// Architecture (the graph model of web/viewer-prototype/graph-diff.mjs) to a
// 3D scene layout. Pure functions: no three.js or DOM, so they can be unit
// tested. The input comes from AI output relayed by the backend and is not
// trusted: unknown layers are dropped, names are plain strings, counts and
// coordinates are capped.
//
//   node: { id, layer: "network"|"code"|"data", kind, name, detail: 1..3,
//           parent?: node id, props?: {...}, pos: {x, z} }
//   edge: { from, to, kind, label? }
//
// Kinds: ingress, service, netpol (network); app, module, external (code);
// database, table, queue, storage (data). Unknown kinds draw as modules.

export const LAYERS = ['network', 'code', 'data'] as const;

export type Layer = typeof LAYERS[number];

// Same vertical spacing as the prototype.
export const LAYER_Y: Record<Layer, number> = {
  network: 8, code: 4, data: 0
};

// Half height of each kind's mesh, so it sits on its layer plane.
export const KIND_HEIGHT: Record<string, number> = {
  ingress:  0.75,
  service:  0.25,
  netpol:   0.12,
  app:      0.4,
  module:   0.22,
  database: 0.55,
  table:    0.15,
  queue:    0.35,
  external: 0.6,
  storage:  0.18,
};

export const MAX_NODES = 100;
export const MAX_EDGES = 300;
export const MAX_NAME_LENGTH = 80;
export const MAX_LABEL_LENGTH = 60;
const MAX_COORD = 100;
const SPACING_X = 4;
const CHILD_SPACING_X = 2;
const CHILD_OFFSET_Z = 2.5;
const MIN_PLANE_W = 12;
const MIN_PLANE_D = 8;
const PLANE_MARGIN = 4;

export interface ArchNode {
  id: string;
  layer: string;
  kind?: string;
  name?: string;
  detail?: number;
  parent?: string;
  props?: Record<string, unknown>;
  pos?: { x?: number; z?: number };
}

export interface ArchEdge {
  from: string;
  to: string;
  kind?: string;
  label?: string;
}

export interface Architecture {
  nodes?: ArchNode[];
  edges?: ArchEdge[];
}

export interface SceneNode {
  id: string;
  layer: Layer;
  kind: string;
  name: string;
  x: number;
  y: number;
  z: number;
}

export interface SceneEdge {
  from: string;
  to: string;
  kind: string;
  label: string;
  sameLayer: boolean;
}

export interface SceneLayout {
  nodes: SceneNode[];
  edges: SceneEdge[];
  plane: { width: number; depth: number; centerX: number; centerZ: number };
}

function isLayer(v: unknown): v is Layer {
  return typeof v === 'string' && (LAYERS as readonly string[]).includes(v);
}

function text(v: unknown, max: number): string {
  if (typeof v !== 'string' && typeof v !== 'number') {
    return '';
  }
  const s = String(v).replace(/\s+/g, ' ').trim();

  return s.length > max ? `${ s.slice(0, max - 1) }…` : s;
}

function validPos(p: unknown): p is { x: number; z: number } {
  if (!p || typeof p !== 'object') {
    return false;
  }
  const { x, z } = p as { x?: unknown; z?: unknown };

  return typeof x === 'number' && typeof z === 'number' &&
    Number.isFinite(x) && Number.isFinite(z) &&
    Math.abs(x) <= MAX_COORD && Math.abs(z) <= MAX_COORD;
}

interface Cleaned {
  id: string;
  layer: Layer;
  kind: string;
  name: string;
  parent: string;
  pos: { x: number; z: number } | null;
}

/**
 * Fallback placement when the backend did not give usable positions:
 * per layer, top-level nodes in a row along x, children in a row behind
 * their parent.
 */
function autoPlace(nodes: Cleaned[]): Map<string, { x: number; z: number }> {
  const out = new Map<string, { x: number; z: number }>();

  for (const layer of LAYERS) {
    const inLayer = nodes.filter((n) => n.layer === layer);
    const ids = new Set(inLayer.map((n) => n.id));
    const roots = inLayer.filter((n) => !n.parent || !ids.has(n.parent) || n.parent === n.id);
    const rootIds = new Set(roots.map((n) => n.id));

    roots.forEach((n, i) => {
      out.set(n.id, { x: (i - (roots.length - 1) / 2) * SPACING_X, z: 0 });
    });

    // Children of a root; deeper nesting is placed with its nearest placed ancestor's row.
    const rest = inLayer.filter((n) => !rootIds.has(n.id));
    const byParent = new Map<string, Cleaned[]>();

    for (const n of rest) {
      const list = byParent.get(n.parent) || [];

      list.push(n);
      byParent.set(n.parent, list);
    }
    let row = 1;
    let pending = rest.slice();

    while (pending.length) {
      const placedNow: Cleaned[] = [];

      for (const [parent, kids] of byParent) {
        const p = out.get(parent);

        if (!p) {
          continue;
        }
        kids.forEach((k, i) => {
          if (!out.has(k.id)) {
            out.set(k.id, { x: p.x + (i - (kids.length - 1) / 2) * CHILD_SPACING_X, z: p.z + CHILD_OFFSET_Z });
            placedNow.push(k);
          }
        });
      }
      pending = pending.filter((n) => !out.has(n.id));
      if (!placedNow.length) {
        // Parent cycle or unreachable parent: put the rest in their own row.
        pending.forEach((n, i) => out.set(n.id, { x: (i - (pending.length - 1) / 2) * SPACING_X, z: CHILD_OFFSET_Z * (row + 1) }));
        break;
      }
      row++;
    }
  }

  return out;
}

/** Turn an untrusted architecture into positioned scene nodes and edges. */
export function layoutArchitecture(arch: unknown): SceneLayout {
  const a = (arch && typeof arch === 'object' ? arch : {}) as Architecture;
  const rawNodes = Array.isArray(a.nodes) ? a.nodes : [];
  const rawEdges = Array.isArray(a.edges) ? a.edges : [];

  const cleaned: Cleaned[] = [];
  const seen = new Set<string>();

  for (const n of rawNodes) {
    if (cleaned.length >= MAX_NODES) {
      break;
    }
    if (!n || typeof n !== 'object') {
      continue;
    }
    const id = typeof n.id === 'string' ? n.id : '';

    if (!id || seen.has(id) || !isLayer(n.layer)) {
      continue;
    }
    seen.add(id);
    cleaned.push({
      id,
      layer:  n.layer,
      kind:   typeof n.kind === 'string' && n.kind ? n.kind : 'module',
      name:   text(n.name, MAX_NAME_LENGTH) || text(id, MAX_NAME_LENGTH),
      parent: typeof n.parent === 'string' ? n.parent : '',
      pos:    validPos(n.pos) ? { x: n.pos.x, z: n.pos.z } : null,
    });
  }

  // The backend lays out positions (docs/m0-design.md). Use them only if
  // every node has one; mixing given and computed positions could overlap.
  const useGiven = cleaned.length > 0 && cleaned.every((n) => n.pos);
  const placed = useGiven ? new Map(cleaned.map((n) => [n.id, n.pos as { x: number; z: number }])) : autoPlace(cleaned);

  const nodes: SceneNode[] = cleaned.map((n) => {
    const p = placed.get(n.id) || { x: 0, z: 0 };

    return {
      id:    n.id,
      layer: n.layer,
      kind:  n.kind,
      name:  n.name,
      x:     p.x,
      y:     LAYER_Y[n.layer] + (KIND_HEIGHT[n.kind] ?? KIND_HEIGHT.module),
      z:     p.z,
    };
  });

  const byId = new Map(nodes.map((n) => [n.id, n]));
  const edges: SceneEdge[] = [];
  const edgeKeys = new Set<string>();

  for (const e of rawEdges) {
    if (edges.length >= MAX_EDGES) {
      break;
    }
    if (!e || typeof e !== 'object' || typeof e.from !== 'string' || typeof e.to !== 'string' || e.from === e.to) {
      continue;
    }
    const from = byId.get(e.from);
    const to = byId.get(e.to);

    if (!from || !to) {
      continue;
    }
    const kind = typeof e.kind === 'string' ? e.kind : '';
    const key = `${ e.from }\u0000${ e.to }\u0000${ kind }`;

    if (edgeKeys.has(key)) {
      continue;
    }
    edgeKeys.add(key);
    edges.push({
      from:      e.from,
      to:        e.to,
      kind,
      label:     text(e.label, MAX_LABEL_LENGTH),
      sameLayer: from.layer === to.layer,
    });
  }

  let minX = 0;
  let maxX = 0;
  let minZ = 0;
  let maxZ = 0;

  if (nodes.length) {
    minX = Math.min(...nodes.map((n) => n.x));
    maxX = Math.max(...nodes.map((n) => n.x));
    minZ = Math.min(...nodes.map((n) => n.z));
    maxZ = Math.max(...nodes.map((n) => n.z));
  }

  return {
    nodes,
    edges,
    plane: {
      width:   Math.max(MIN_PLANE_W, maxX - minX + PLANE_MARGIN),
      depth:   Math.max(MIN_PLANE_D, maxZ - minZ + PLANE_MARGIN),
      centerX: (minX + maxX) / 2,
      centerZ: (minZ + maxZ) / 2,
    },
  };
}

export type Positions = Record<string, { x: number; z: number }>;

/** The architecture with dragged positions applied (not yet saved). */
export function withPositions(arch: unknown, positions: Positions | null): unknown {
  if (!positions || !Object.keys(positions).length || !arch || typeof arch !== 'object') {
    return arch;
  }
  const a = arch as { nodes?: unknown };

  if (!Array.isArray(a.nodes)) {
    return arch;
  }

  return {
    ...a,
    nodes: a.nodes.map((n) => {
      const id = n && typeof n === 'object' ? (n as { id?: unknown }).id : undefined;

      return typeof id === 'string' && Object.prototype.hasOwnProperty.call(positions, id) ? { ...n, pos: positions[id] } : n;
    }),
  };
}
