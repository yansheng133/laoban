import {
  describe, it, expect, beforeEach
} from '@jest/globals';
import {
  findTarget, setTarget, namespaceFrom, apiBase, BackendNotFound, Probe, ProbeAnswer
} from '../target';

const GV = '/apis/api.laoban.yansheng133.github.io/v1alpha1';

// A Rancher with these clusters; `laoban` names the one running the
// backend and the namespace it serves; `visible` the user's namespaces.
function rancher(opts: { clusters: string[]; laoban?: { cluster: string; namespace: string }; visible?: string[]; editIn?: string[] }) {
  const asked: string[] = [];
  const probe: Probe = async(url) => {
    asked.push(url);
    const answer = (status: number, body: unknown = {}): ProbeAnswer => ({ status, body });

    if (url === '/v1/management.cattle.io.clusters') {
      return answer(200, { data: opts.clusters.map((id) => ({ id })) });
    }
    const m = /^\/k8s\/clusters\/([^/]+)(.*)$/.exec(url);

    if (!m) {
      return answer(404);
    }
    const [, cluster, rest] = m;
    const here = opts.laoban?.cluster === cluster;

    if (rest === GV) {
      return answer(here ? 200 : 404);
    }
    if (rest === '/v1/namespaces') {
      return answer(200, { data: (opts.visible || []).map((id) => ({ id })) });
    }
    const ns = new RegExp(`^${ GV }/namespaces/([^/]+)/config$`).exec(rest)?.[1];

    if (!here || !ns) {
      return answer(404);
    }
    if (ns === opts.laoban!.namespace) {
      return answer(200, { model: 'm' });
    }
    // The API server lets the request through where the user may use laoban.
    if ((opts.editIn || []).includes(ns)) {
      return answer(404, { error: { code: 'other_namespace', targetNamespace: opts.laoban!.namespace } });
    }

    return answer(403, { kind: 'Status' });
  };

  return { probe, asked };
}

describe('findTarget', () => {
  beforeEach(() => setTarget(null));

  it('finds the cluster with laoban’s API and its default namespace', async() => {
    const r = rancher({ clusters: ['local', 'c-m-1', 'c-m-2'], laoban: { cluster: 'c-m-2', namespace: 'laoban-dev' } });

    expect(await findTarget(r.probe)).toStrictEqual({ clusterId: 'c-m-2', namespace: 'laoban-dev' });
    // Kept: not looked for again.
    const n = r.asked.length;

    await findTarget(r.probe);
    expect(r.asked).toHaveLength(n);
  });

  it('follows the backend to the namespace it serves', async() => {
    const r = rancher({
      clusters: ['c-m-9'], laoban: { cluster: 'c-m-9', namespace: 'team-apps' }, visible: ['default', 'team-apps'], editIn: ['laoban-dev']
    });

    expect(await findTarget(r.probe)).toStrictEqual({ clusterId: 'c-m-9', namespace: 'team-apps' });
  });

  it('asks the user’s namespaces when the default one says nothing', async() => {
    const r = rancher({
      clusters: ['c-m-9'], laoban: { cluster: 'c-m-9', namespace: 'apps-b' }, visible: ['a', 'b', 'apps-b']
    });

    expect(await findTarget(r.probe)).toStrictEqual({ clusterId: 'c-m-9', namespace: 'apps-b' });
  });

  it('says so when no cluster has it', async() => {
    const r = rancher({ clusters: ['local', 'c-m-1'] });

    await expect(findTarget(r.probe)).rejects.toBeInstanceOf(BackendNotFound);
  });
});

describe('namespaceFrom', () => {
  it('reads the answer to a config request', () => {
    expect(namespaceFrom('x', { status: 200, body: {} })).toBe('x');
    expect(namespaceFrom('x', { status: 404, body: { error: { code: 'other_namespace', targetNamespace: 'y' } } })).toBe('y');
    expect(namespaceFrom('x', { status: 404, body: { error: { code: 'bad_request' } } })).toBeNull();
    expect(namespaceFrom('x', { status: 403, body: null })).toBeNull();
  });
});

describe('apiBase', () => {
  it('is the aggregated API under the namespace, through Rancher', () => {
    expect(apiBase({ clusterId: 'c-m-1', namespace: 'laoban-dev' }))
      .toBe('/k8s/clusters/c-m-1/apis/api.laoban.yansheng133.github.io/v1alpha1/namespaces/laoban-dev');
  });
});
