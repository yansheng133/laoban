import { describe, it, expect } from '@jest/globals';
import { bundleReadiness, Bundle, FLEET_COMMIT_LABEL } from '../bundle';

const PREFIX = 'laoban-apps-apps-breakfast';

function bundle(name: string, opts: {
  ready?: number; desired?: number; errApplied?: number; modified?: number; readyCond?: string; message?: string;
  generation?: number; observed?: number; commit?: string;
} = {}): Bundle {
  return {
    metadata: {
      name,
      generation: opts.generation ?? 1,
      labels:     opts.commit ? { [FLEET_COMMIT_LABEL]: opts.commit } : {},
    },
    status: {
      observedGeneration: opts.observed ?? 1,
      conditions:         opts.readyCond ? [{
        type: 'Ready', status: opts.readyCond, message: opts.message
      }] : [],
      summary:            {
        desiredReady: opts.desired ?? 1, ready: opts.ready ?? 0, errApplied: opts.errApplied ?? 0, modified: opts.modified ?? 0
      },
    },
  };
}

describe('bundleReadiness', () => {
  it('is missing when no bundle has the prefix', () => {
    const r = bundleReadiness([bundle('other-app', { ready: 1 })], PREFIX);

    expect(r.state).toBe('missing');
    expect(r.matched).toBe(0);
  });

  it('is missing when the prefix is empty', () => {
    expect(bundleReadiness([bundle(PREFIX, { ready: 1 })], '').state).toBe('missing');
  });

  it('is ready when ready equals desiredReady and the Ready condition is True', () => {
    const r = bundleReadiness([bundle(PREFIX, { ready: 1, readyCond: 'True' })], PREFIX);

    expect(r).toEqual({
      state: 'ready', matched: 1, ready: 1, desired: 1, message: ''
    });
  });

  it('is ready without a Ready condition when the summary is complete', () => {
    expect(bundleReadiness([bundle(PREFIX, { ready: 1 })], PREFIX).state).toBe('ready');
  });

  it('is pending while not all targets are ready, and carries the condition message', () => {
    const r = bundleReadiness([bundle(PREFIX, {
      ready: 0, readyCond: 'False', message: 'NotReady(1) [Cluster fleet-default/rke2]'
    })], PREFIX);

    expect(r.state).toBe('pending');
    expect(r.message).toBe('NotReady(1) [Cluster fleet-default/rke2]');
  });

  it('is pending when the Ready condition is False even if counts match', () => {
    expect(bundleReadiness([bundle(PREFIX, { ready: 1, readyCond: 'False' })], PREFIX).state).toBe('pending');
  });

  it('is pending when desiredReady is 0 (no target yet)', () => {
    expect(bundleReadiness([bundle(PREFIX, { ready: 0, desired: 0 })], PREFIX).state).toBe('pending');
  });

  it('is pending when the status has not caught up with the spec', () => {
    expect(bundleReadiness([bundle(PREFIX, {
      ready: 1, generation: 3, observed: 2
    })], PREFIX).state).toBe('pending');
  });

  it('is pending when the bundle was built from another commit', () => {
    const b = bundle(PREFIX, { ready: 1, commit: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' });

    expect(bundleReadiness([b], PREFIX, 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb').state).toBe('pending');
    expect(bundleReadiness([b], PREFIX, 'aaaaaaa').state).toBe('ready');
  });

  it('reports error when Fleet counts apply errors', () => {
    expect(bundleReadiness([bundle(PREFIX, { errApplied: 1 })], PREFIX).state).toBe('error');
  });

  it('needs every matching bundle to be ready', () => {
    const bundles = [bundle(PREFIX, { ready: 1 }), bundle(`${ PREFIX }-extra`, { ready: 0, desired: 1 })];
    const r = bundleReadiness(bundles, PREFIX);

    expect(r.state).toBe('pending');
    expect(r.matched).toBe(2);
    expect(r.ready).toBe(1);
    expect(r.desired).toBe(2);
  });

  it('ignores malformed entries', () => {
    const bundles = [null, {}, { metadata: {} }, bundle(PREFIX, { ready: 1 })] as unknown as Bundle[];

    expect(bundleReadiness(bundles, PREFIX).state).toBe('ready');
  });

  // Seen in the first M0 run: Helm rewrote a label, so Fleet reported
  // Modified(1) forever although the app was running.
  it('is modified when every target is applied but drifted from Git', () => {
    const msg = 'Modified(1) [Cluster fleet-default/rke2]; deployment.apps laoban-dev/x-web modified';
    const r = bundleReadiness([bundle(PREFIX, {
      ready: 0, modified: 1, readyCond: 'False', message: msg
    })], PREFIX);

    expect(r.state).toBe('modified');
    expect(r.message).toBe(msg);
  });

  it('is not modified while some target is still pending', () => {
    expect(bundleReadiness([bundle(PREFIX, {
      desired: 2, ready: 0, modified: 1, readyCond: 'False'
    })], PREFIX).state).toBe('pending');
  });

  it('prefers error over modified', () => {
    expect(bundleReadiness([bundle(PREFIX, {
      ready: 0, modified: 1, errApplied: 1
    })], PREFIX).state).toBe('error');
  });

  it('is modified when one bundle is ready and another only modified', () => {
    expect(bundleReadiness([
      bundle(`${ PREFIX }-a`, { ready: 1 }),
      bundle(`${ PREFIX }-b`, { ready: 0, modified: 1 }),
    ], PREFIX).state).toBe('modified');
  });
});
