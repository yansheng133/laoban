import { describe, it, expect } from '@jest/globals';
import { mergeForDiff, edgeKey, REMOVED_OFFSET_Z } from '../graphDiff';
import type { Architecture } from '../layout';

const base: Architecture = {
  nodes: [
    {
      id: 'app-web', layer: 'code', kind: 'app', name: 'shop-web', pos: { x: 0, z: 0 }
    },
    {
      id: 'mod-web-menu', layer: 'code', kind: 'module', name: 'menu', parent: 'app-web', pos: { x: 0, z: 2.5 }
    },
    {
      id: 'mod-web-cart', layer: 'code', kind: 'module', name: 'cart', parent: 'app-web', pos: { x: 2, z: 2.5 }
    },
    {
      id: 'app-api', layer: 'code', kind: 'app', name: 'shop-api', props: { routes: 'GET /menu' }, pos: { x: 6, z: 0 }
    },
  ],
  edges: [
    { from: 'app-web', to: 'app-api', kind: 'calls', label: 'HTTP api/*' },
    { from: 'app-api', to: 'tbl', kind: 'writes' },
  ],
};

const target: Architecture = {
  nodes: [
    {
      id: 'app-web', layer: 'code', kind: 'app', name: 'shop-web', pos: { x: 0, z: 0 }
    },
    {
      id: 'mod-web-menu', layer: 'code', kind: 'module', name: 'menu', parent: 'app-web', pos: { x: 1, z: 2.5 }
    },
    {
      id: 'mod-web-drinks', layer: 'code', kind: 'module', name: 'drinks', parent: 'app-web', pos: { x: 2, z: 2.5 }
    },
    {
      id: 'app-api', layer: 'code', kind: 'app', name: 'shop-api', props: { routes: 'GET /drinks, GET /menu' }, pos: { x: 6, z: 0 }
    },
  ],
  edges: [
    { from: 'app-web', to: 'app-api', kind: 'calls', label: 'HTTP api/*' },
    { from: 'app-api', to: 'cache', kind: 'reads' },
  ],
};

describe('mergeForDiff', () => {
  const d = mergeForDiff(base, target);

  it('marks each node', () => {
    expect(d.nodes).toEqual({
      'app-web':        'unchanged',
      'mod-web-menu':   'moved',
      'mod-web-drinks': 'added',
      'app-api':        'changed',
      'mod-web-cart':   'removed',
    });
  });

  it('marks each edge by from/to/kind', () => {
    expect(d.edges[edgeKey({ from: 'app-web', to: 'app-api', kind: 'calls' })]).toBe('unchanged');
    expect(d.edges[edgeKey({ from: 'app-api', to: 'cache', kind: 'reads' })]).toBe('added');
    expect(d.edges[edgeKey({ from: 'app-api', to: 'tbl', kind: 'writes' })]).toBe('removed');
  });

  it('keeps removed parts in the merged graph, set back a little', () => {
    const cart = d.architecture.nodes!.find((n) => n.id === 'mod-web-cart');

    expect(cart?.pos).toEqual({ x: 2, z: 2.5 + REMOVED_OFFSET_Z });
    expect(d.architecture.nodes).toHaveLength(5);
    expect(d.architecture.edges).toHaveLength(3);
  });

  it('counts architecture changes, not moves', () => {
    expect(d.summary).toEqual({ added: 2, removed: 2, changed: 1 });
  });

  it('treats a missing side as empty', () => {
    expect(mergeForDiff(null, target).summary.added).toBe(6);
    expect(mergeForDiff(base, undefined).summary.removed).toBe(6);
    expect(mergeForDiff(target, target).summary).toEqual({ added: 0, removed: 0, changed: 0 });
  });
});
