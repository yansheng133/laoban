import { describe, it, expect } from '@jest/globals';
import {
  layoutArchitecture, withPositions, LAYER_Y, KIND_HEIGHT, MAX_NODES, MAX_NAME_LENGTH
} from '../layout';

// The fixed M0 shape from docs/m0-design.md.
const M0 = {
  nodes: [
    {
      id: 'svc-web', layer: 'network', kind: 'service', name: 'web', detail: 1, pos: { x: -3, z: 0 }
    },
    {
      id: 'svc-api', layer: 'network', kind: 'service', name: 'api', detail: 1, pos: { x: 3, z: 0 }
    },
    {
      id: 'app-web', layer: 'code', kind: 'app', name: 'web', detail: 1, pos: { x: -3, z: 0 }
    },
    {
      id: 'app-api', layer: 'code', kind: 'app', name: 'api', detail: 1, pos: { x: 3, z: 0 }
    },
    {
      id: 'mod-menu', layer: 'code', kind: 'module', name: 'menu', detail: 3, parent: 'app-web', pos: { x: -3, z: 2.5 }
    },
    {
      id: 'tbl-orders', layer: 'data', kind: 'table', name: 'orders', detail: 2, props: { storage: 'memory' }, pos: { x: 3, z: 0 }
    },
  ],
  edges: [
    {
      from: 'svc-web', to: 'app-web', kind: 'routes'
    },
    {
      from: 'app-web', to: 'svc-api', kind: 'calls', label: 'POST /orders'
    },
    {
      from: 'svc-api', to: 'app-api', kind: 'routes'
    },
    {
      from: 'app-api', to: 'tbl-orders', kind: 'writes'
    },
  ],
};

describe('layoutArchitecture', () => {
  it('uses backend positions and puts each node on its layer', () => {
    const l = layoutArchitecture(M0);

    expect(l.nodes).toHaveLength(6);
    const web = l.nodes.find((n) => n.id === 'svc-web');

    expect(web).toMatchObject({
      x: -3, z: 0, y: LAYER_Y.network + KIND_HEIGHT.service
    });
    const tbl = l.nodes.find((n) => n.id === 'tbl-orders');

    expect(tbl?.y).toBe(LAYER_Y.data + KIND_HEIGHT.table);
    expect(l.edges).toHaveLength(4);
    expect(l.edges.find((e) => e.label)).toMatchObject({
      from: 'app-web', to: 'svc-api', label: 'POST /orders', sameLayer: false
    });
  });

  it('sizes the planes to the nodes with a minimum size', () => {
    const l = layoutArchitecture(M0);

    expect(l.plane.width).toBeGreaterThanOrEqual(12);
    expect(l.plane.depth).toBeGreaterThanOrEqual(8);
    expect(l.plane.centerX).toBe(0);
    expect(l.plane.centerZ).toBe(1.25);
  });

  it('lays out nodes itself when any position is missing or invalid', () => {
    const arch = {
      nodes: [
        {
          id: 'a', layer: 'code', kind: 'app', name: 'a'
        },
        {
          id: 'b', layer: 'code', kind: 'app', name: 'b', pos: { x: 'x', z: 0 }
        },
        {
          id: 'c', layer: 'code', kind: 'module', name: 'c', parent: 'a', pos: { x: 1e9, z: 0 }
        },
      ],
      edges: [],
    };
    const l = layoutArchitecture(arch);
    const pos = Object.fromEntries(l.nodes.map((n) => [n.id, [n.x, n.z]]));

    expect(pos.a).toEqual([-2, 0]);
    expect(pos.b).toEqual([2, 0]);
    // child sits behind its parent
    expect(pos.c).toEqual([-2, 2.5]);
  });

  it('drops nodes with unknown layers, duplicate ids, and edges to missing nodes', () => {
    const l = layoutArchitecture({
      nodes: [
        {
          id: 'a', layer: 'code', name: 'a'
        },
        {
          id: 'a', layer: 'data', name: 'dup'
        },
        {
          id: 'x', layer: 'secrets', name: 'x'
        },
        { layer: 'code', name: 'no id' },
      ],
      edges: [
        {
          from: 'a', to: 'x', kind: 'k'
        },
        {
          from: 'a', to: 'a', kind: 'self'
        },
        {
          from: 'a', to: 'missing', kind: 'k'
        },
      ],
    });

    expect(l.nodes.map((n) => n.id)).toEqual(['a']);
    expect(l.nodes[0].kind).toBe('module');
    expect(l.edges).toEqual([]);
  });

  it('keeps names as plain, length-limited text', () => {
    const l = layoutArchitecture({
      nodes: [
        {
          id: 'a', layer: 'code', name: '<img src=x onerror=alert(1)>'
        },
        {
          id: 'b', layer: 'code', name: 'x'.repeat(500)
        },
        {
          id: 'c', layer: 'code', name: { toString: () => 'obj' }
        },
      ],
    });

    expect(l.nodes[0].name).toBe('<img src=x onerror=alert(1)>');
    expect(l.nodes[1].name.length).toBe(MAX_NAME_LENGTH);
    expect(l.nodes[2].name).toBe('c');
  });

  it('caps the number of nodes', () => {
    const nodes = Array.from({ length: MAX_NODES + 20 }, (_, i) => ({
      id: `n${ i }`, layer: 'code', name: `n${ i }`
    }));

    expect(layoutArchitecture({ nodes }).nodes).toHaveLength(MAX_NODES);
  });

  it('returns an empty layout for garbage input', () => {
    for (const input of [null, undefined, 'x', 42, { nodes: 'no' }]) {
      const l = layoutArchitecture(input);

      expect(l.nodes).toEqual([]);
      expect(l.edges).toEqual([]);
      expect(l.plane.width).toBe(12);
    }
  });
});

describe('withPositions', () => {
  const arch = {
    nodes: [
      {
        id: 'a', layer: 'code', kind: 'module', pos: { x: 0, z: 0 }
      },
      {
        id: 'b', layer: 'code', kind: 'external', pos: { x: 4, z: 0 }
      },
      {
        id: 'c', layer: 'data', kind: 'storage', pos: { x: 0, z: 2 }
      },
    ],
  };

  it('overrides only the given nodes', () => {
    const l = layoutArchitecture(withPositions(arch, { b: { x: 7, z: -3 } }));

    expect(l.nodes.find((n) => n.id === 'b')).toMatchObject({ x: 7, z: -3 });
    expect(l.nodes.find((n) => n.id === 'a')).toMatchObject({ x: 0, z: 0 });
  });

  it('returns the same object without positions', () => {
    expect(withPositions(arch, null)).toBe(arch);
    expect(withPositions(arch, {})).toBe(arch);
    expect(withPositions('x', { a: { x: 1, z: 1 } })).toBe('x');
  });

  it('places the external and storage kinds on their layers', () => {
    const l = layoutArchitecture(arch);

    expect(l.nodes.find((n) => n.id === 'b')?.y).toBeCloseTo(4 + KIND_HEIGHT.external);
    expect(l.nodes.find((n) => n.id === 'c')?.y).toBeCloseTo(0 + KIND_HEIGHT.storage);
  });
});
