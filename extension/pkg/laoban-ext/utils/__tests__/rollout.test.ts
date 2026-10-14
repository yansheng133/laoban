import { describe, it, expect } from '@jest/globals';
import {
  deploymentRolledOut, rolloutTargets, Deployment, CONTENT_HASH_ANNOTATION
} from '../rollout';

const HASH = 'a'.repeat(64);

function dep(opts: {
  hash?: string; gen?: number; observed?: number; replicas?: number;
  total?: number; updated?: number; available?: number; unavailable?: number;
} = {}): Deployment {
  return {
    metadata: { generation: opts.gen ?? 2 },
    spec:     {
      replicas: opts.replicas ?? 1,
      template: { metadata: { annotations: { [CONTENT_HASH_ANNOTATION]: opts.hash ?? HASH } } },
    },
    status: {
      observedGeneration:  opts.observed ?? 2,
      replicas:            opts.total ?? 1,
      updatedReplicas:     opts.updated ?? 1,
      availableReplicas:   opts.available ?? 1,
      unavailableReplicas: opts.unavailable,
    },
  };
}

describe('deploymentRolledOut', () => {
  it('is true for the new template, fully rolled out', () => {
    expect(deploymentRolledOut(dep(), HASH)).toBe(true);
  });

  it('is false while the old template is still in place', () => {
    expect(deploymentRolledOut(dep({ hash: 'b'.repeat(64) }), HASH)).toBe(false);
  });

  it('is false until the controller has seen the new generation', () => {
    expect(deploymentRolledOut(dep({ gen: 3, observed: 2 }), HASH)).toBe(false);
  });

  it('is false while old pods remain or new ones are not available', () => {
    expect(deploymentRolledOut(dep({ total: 2, updated: 1 }), HASH)).toBe(false);
    expect(deploymentRolledOut(dep({ available: 0 }), HASH)).toBe(false);
    expect(deploymentRolledOut(dep({ unavailable: 1 }), HASH)).toBe(false);
  });

  it('is false for a missing deployment or hash', () => {
    expect(deploymentRolledOut(null, HASH)).toBe(false);
    expect(deploymentRolledOut(dep(), '')).toBe(false);
  });
});

describe('rolloutTargets', () => {
  it('keeps well-formed targets and drops the rest', () => {
    const good = { deployment: 'todo-web', path: '/k8s/clusters/c-m-1/v1/apps.deployments/laoban-dev/todo-web', contentHash: HASH };

    expect(rolloutTargets([good, { deployment: 'x', path: 'https://evil.example/x', contentHash: HASH }, null, 3])).toEqual([good]);
    expect(rolloutTargets(undefined)).toEqual([]);
  });
});
