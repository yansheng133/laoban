// Fleet Bundle readiness for the app that /api/apply just committed.
// Pure functions: no browser or store access, so they can be unit tested.

export interface BundleCondition {
  type?: string;
  status?: string;
  message?: string;
  reason?: string;
}

export interface Bundle {
  metadata?: {
    name?: string;
    generation?: number;
    labels?: Record<string, string>;
    // Steve adds a computed state to every object.
    state?: { name?: string; error?: boolean; transitioning?: boolean; message?: string };
  };
  status?: {
    observedGeneration?: number;
    conditions?: BundleCondition[];
    summary?: {
      desiredReady?: number;
      ready?: number;
      errApplied?: number;
      notReady?: number;
      waitApplied?: number;
      outOfSync?: number;
      modified?: number;
      pending?: number;
    };
  };
}

// 'missing': no bundle with the prefix yet (Fleet has not picked up the commit)
// 'pending': found, still rolling out
// 'error':   Fleet reports an apply error (it may still recover)
// 'modified': applied everywhere, but Fleet sees the live objects differ from
//             Git (its "Modified" state). The app is running, so the page can
//             offer the link, with a warning.
// 'ready':   every matching bundle is ready on every target
export type ReadinessState = 'missing' | 'pending' | 'error' | 'modified' | 'ready';

export interface Readiness {
  state: ReadinessState;
  matched: number;
  ready: number;
  desired: number;
  message: string;
}

// Label Fleet may put on bundles with the Git commit they were built from.
export const FLEET_COMMIT_LABEL = 'fleet.cattle.io/commit';

function sameCommit(a: string, b: string): boolean {
  const x = a.trim().toLowerCase();
  const y = b.trim().toLowerCase();

  if (x.length < 7 || y.length < 7) {
    return x === y;
  }

  return x.startsWith(y) || y.startsWith(x);
}

/** Readiness of a single bundle. */
export function bundleState(b: Bundle, commit?: string): { state: Exclude<ReadinessState, 'missing'>; ready: number; desired: number; message: string } {
  const summary = b.status?.summary || {};
  const desired = Number(summary.desiredReady) || 0;
  const ready = Number(summary.ready) || 0;
  const readyCond = (b.status?.conditions || []).find((c) => c?.type === 'Ready');
  const message = String(readyCond?.message || b.metadata?.state?.message || '');

  // Built from an older commit: the new one has not been applied yet.
  const label = b.metadata?.labels?.[FLEET_COMMIT_LABEL];

  if (commit && label && !sameCommit(label, commit)) {
    return {
      state: 'pending', ready, desired, message
    };
  }

  // Status not yet reconciled for the current spec.
  const gen = b.metadata?.generation;
  const observed = b.status?.observedGeneration;

  if (typeof gen === 'number' && (typeof observed !== 'number' || observed < gen)) {
    return {
      state: 'pending', ready, desired, message
    };
  }

  // Only Fleet's own counter; Steve's metadata.state.error can also be set
  // while a rollout is merely in progress.
  if ((Number(summary.errApplied) || 0) > 0) {
    return {
      state: 'error', ready, desired, message
    };
  }

  const condOk = !readyCond || readyCond.status === 'True';

  if (desired > 0 && ready >= desired && condOk) {
    return {
      state: 'ready', ready, desired, message: ''
    };
  }

  // Every target is either ready or only modified (nothing pending or
  // failing): Fleet will not get any further on its own.
  const modified = Number(summary.modified) || 0;

  if (desired > 0 && modified > 0 && ready + modified >= desired) {
    return {
      state: 'modified', ready, desired, message
    };
  }

  return {
    state: 'pending', ready, desired, message
  };
}

/**
 * Combined readiness of all bundles whose name starts with `prefix`
 * (the `fleet.bundlePrefix` from POST /api/apply).
 */
export function bundleReadiness(bundles: Bundle[], prefix: string, commit?: string): Readiness {
  const matching = prefix ? bundles.filter((b) => (b?.metadata?.name || '').startsWith(prefix)) : [];

  if (!matching.length) {
    return {
      state: 'missing', matched: 0, ready: 0, desired: 0, message: ''
    };
  }

  const states = matching.map((b) => bundleState(b, commit));
  const ready = states.reduce((n, s) => n + s.ready, 0);
  const desired = states.reduce((n, s) => n + s.desired, 0);
  const message = states.map((s) => s.message).find(Boolean) || '';
  let state: ReadinessState = 'pending';

  if (states.every((s) => s.state === 'ready')) {
    state = 'ready';
  } else if (states.some((s) => s.state === 'error')) {
    state = 'error';
  } else if (states.every((s) => s.state === 'ready' || s.state === 'modified')) {
    state = 'modified';
  }

  return {
    state, matched: matching.length, ready, desired, message
  };
}
