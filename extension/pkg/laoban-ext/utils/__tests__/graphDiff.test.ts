import { describe, it, expect } from '@jest/globals';
import {
  mergeForDiff, edgeKey, REMOVED_OFFSET_Z, diffEntries, moduleFile
} from '../graphDiff';
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

describe('diffEntries', () => {
  it('lists the marked parts: added, changed, removed; components first', () => {
    const list = diffEntries(mergeForDiff(base, target));

    expect(list.map((e) => [e.status, e.type, e.label])).toEqual([
      ['added', 'node', 'drinks'],
      ['added', 'edge', 'shop-api → cache'],
      ['changed', 'node', 'shop-api'],
      ['removed', 'node', 'cart'],
      ['removed', 'edge', 'shop-api → tbl'],
    ]);
    // A connection selects its source component.
    expect(list[1].nodeId).toBe('app-api');
  });

  it('is empty without a comparison or without changes', () => {
    expect(diffEntries(null)).toEqual([]);
    expect(diffEntries(mergeForDiff(target, target))).toEqual([]);
  });
});

describe('comparing with the previous version', () => {
  const web = (purpose: string) => ({
    nodes: [
      { id: 'app-web', layer: 'code', kind: 'app', name: 'web' },
      { id: 'app-api', layer: 'code', kind: 'app', name: 'api' },
      { id: 'w-app', layer: 'code', kind: 'module', name: 'app.js', parent: 'app-web', props: { purpose } },
      { id: 'w-ui', layer: 'code', kind: 'module', name: 'board_ui', parent: 'app-web' },
      { id: 'a-board', layer: 'code', kind: 'module', name: 'message_board_api.py', parent: 'app-api' },
    ],
    edges: [],
  });

  it('a version that changed only code has no architecture changes against the one before it', () => {
    // A hand edit of index.html: same plan, so the same architecture.
    expect(diffEntries(mergeForDiff(web('list'), web('list')))).toEqual([]);
  });

  it('lists only what that version changed, not what differs from the current one', () => {
    const v8 = web('list');
    const v9 = web('list');
    const current = web('list and sort');

    expect(diffEntries(mergeForDiff(v8, v9))).toEqual([]);
    expect(diffEntries(mergeForDiff(current, v9)).map((e) => e.label)).toEqual(['app.js']);
  });

  it('says which file holds a module named by the AI', () => {
    const a = web('x');
    const byName = (n: string) => a.nodes.find((x) => x.name === n);

    expect(moduleFile(byName('message_board_api.py'))).toBe('api');
    expect(moduleFile(byName('board_ui'))).toBe('web');
    expect(moduleFile(byName('app.js'))).toBe('');
    expect(moduleFile(byName('web'))).toBe('');
    expect(moduleFile({ kind: 'module', parent: 'app-api', name: 'handler.py' })).toBe('');
    expect(moduleFile(undefined)).toBe('');
  });
});
