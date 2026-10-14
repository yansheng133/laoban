// Is the version laoban just committed the one actually running?
//
// Fleet can report a bundle Ready for the new commit a little before the
// Deployment has rolled out its new pods (seen in the overnight run), so
// "open app" could show the old version. After the bundle is ready the page
// also checks each Deployment: its pod template must carry the
// laoban.io/content-hash that /api/apply returned, and the rollout must be
// complete (no old pods left). Pure functions, unit tested.

export const CONTENT_HASH_ANNOTATION = 'laoban.io/content-hash';

export interface RolloutTarget {
  deployment: string;
  // Steve path of the Deployment through the Rancher proxy.
  path: string;
  contentHash: string;
}

export interface Deployment {
  metadata?: { generation?: number };
  spec?: {
    replicas?: number;
    template?: { metadata?: { annotations?: Record<string, string> } };
  };
  status?: {
    observedGeneration?: number;
    replicas?: number;
    updatedReplicas?: number;
    availableReplicas?: number;
    unavailableReplicas?: number;
  };
}

/** True when `d` runs the template with `hash` and has finished rolling out. */
export function deploymentRolledOut(d: Deployment | null | undefined, hash: string): boolean {
  if (!d || !hash) {
    return false;
  }
  if (d.spec?.template?.metadata?.annotations?.[CONTENT_HASH_ANNOTATION] !== hash) {
    return false;
  }
  const gen = d.metadata?.generation;
  const observed = d.status?.observedGeneration;

  if (typeof gen === 'number' && (typeof observed !== 'number' || observed < gen)) {
    return false;
  }
  const want = typeof d.spec?.replicas === 'number' ? d.spec.replicas : 1;
  const s = d.status || {};
  const updated = s.updatedReplicas || 0;
  const available = s.availableReplicas || 0;
  const total = s.replicas || 0;

  // All pods are new and available, and no old ones are left.
  return updated >= want && available >= want && total <= updated && !(s.unavailableReplicas || 0);
}

/** Validate the `rollout` field of /api/apply; anything malformed is dropped. */
export function rolloutTargets(value: unknown): RolloutTarget[] {
  if (!Array.isArray(value)) {
    return [];
  }

  return value.filter((t): t is RolloutTarget => !!t && typeof t === 'object' &&
    typeof (t as RolloutTarget).deployment === 'string' &&
    typeof (t as RolloutTarget).contentHash === 'string' &&
    typeof (t as RolloutTarget).path === 'string' &&
    /^\/k8s\/clusters\/[a-z0-9-]+\/v1\/apps\.deployments\/[a-z0-9-]+\/[a-z0-9-]+$/.test((t as RolloutTarget).path));
}
