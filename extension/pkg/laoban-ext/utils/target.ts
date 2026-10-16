// Where the laoban backend is: which Rancher-managed cluster runs it and
// which namespace it serves. Found the first time the page needs it, so the
// published extension works on any Rancher, not only the one it was built
// for.
//
// 1. The answer kept from last time (this browser), if it still answers.
// 2. Otherwise every cluster the user can see is asked for laoban's API
//    group (discovery: any signed-in user may read it). On the first that
//    has it, the namespace is found by asking for /config: the default
//    namespace first, then the user's namespaces there. The backend answers
//    a request for another namespace with the one it serves
//    (other_namespace), so usually one request is enough.
import { API_GROUP, API_VERSION, DEFAULT_TARGET_NAMESPACE } from '../config/backend';

export interface BackendTarget {
  clusterId: string;
  namespace: string;
}

export interface ProbeAnswer {
  status: number;
  body: unknown;
}

// One GET; status 0 when it did not get through.
export type Probe = (url: string) => Promise<ProbeAnswer>;

const STORAGE_KEY = 'laoban-backend-target';
const PROBE_TIMEOUT_MS = 8000;
// Namespaces asked at once, and at most, in one cluster.
const BATCH = 8;
const MAX_NAMESPACES = 200;

let current: BackendTarget | null = null;
let finding: Promise<BackendTarget> | null = null;

export class BackendNotFound extends Error {
  constructor() {
    super('backend_not_found');
    this.name = 'BackendNotFound';
  }
}

export function apiBase(t: BackendTarget): string {
  return `/k8s/clusters/${ encodeURIComponent(t.clusterId) }/apis/${ API_GROUP }/${ API_VERSION }/namespaces/${ encodeURIComponent(t.namespace) }`;
}

export function currentTarget(): BackendTarget | null {
  return current;
}

/** Use this target (tests). */
export function setTarget(t: BackendTarget | null) {
  current = t;
  finding = null;
}

/** Forget the answer (when the backend moved). */
export function resetTarget() {
  current = null;
  finding = null;
  try {
    window.localStorage.removeItem(STORAGE_KEY);
  } catch {}
}

function remembered(): BackendTarget | null {
  try {
    const v = JSON.parse(window.localStorage.getItem(STORAGE_KEY) || 'null');

    if (v && typeof v.clusterId === 'string' && typeof v.namespace === 'string' && v.clusterId && v.namespace) {
      return { clusterId: v.clusterId, namespace: v.namespace };
    }
  } catch {}

  return null;
}

function remember(t: BackendTarget) {
  current = t;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(t));
  } catch {}
}

export const fetchProbe: Probe = async(url) => {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), PROBE_TIMEOUT_MS);

  try {
    const res = await fetch(url, {
      headers: { Accept: 'application/json' }, credentials: 'same-origin', signal: controller.signal
    });
    let body: unknown = null;

    try {
      body = await res.json();
    } catch {}

    return { status: res.status, body };
  } catch {
    return { status: 0, body: null };
  } finally {
    clearTimeout(timer);
  }
};

// The namespace this answer to GET .../namespaces/<ns>/config points to:
// <ns> itself when it is laoban's, the one laoban serves when it says so.
export function namespaceFrom(asked: string, a: ProbeAnswer): string | null {
  if (a.status === 200) {
    return asked;
  }
  const err = (a.body as { error?: { code?: unknown; targetNamespace?: unknown } } | null)?.error;

  if (a.status === 404 && err?.code === 'other_namespace' && typeof err.targetNamespace === 'string' && err.targetNamespace) {
    return err.targetNamespace;
  }

  return null;
}

async function namespaceIn(clusterId: string, probe: Probe): Promise<string | null> {
  const ask = async(ns: string) => namespaceFrom(ns, await probe(`${ apiBase({ clusterId, namespace: ns }) }/config`));
  const first = await ask(DEFAULT_TARGET_NAMESPACE);

  if (first) {
    return first;
  }
  const list = await probe(`/k8s/clusters/${ encodeURIComponent(clusterId) }/v1/namespaces`);
  const names = ((list.body as { data?: { id?: unknown }[] } | null)?.data || [])
    .map((n) => n.id)
    .filter((id): id is string => typeof id === 'string' && id !== DEFAULT_TARGET_NAMESPACE)
    .slice(0, MAX_NAMESPACES);

  for (let i = 0; i < names.length; i += BATCH) {
    const found = (await Promise.all(names.slice(i, i + BATCH).map(ask))).find((n) => n);

    if (found) {
      return found;
    }
  }

  return null;
}

async function search(probe: Probe): Promise<BackendTarget> {
  const kept = remembered();

  if (kept) {
    const ns = namespaceFrom(kept.namespace, await probe(`${ apiBase(kept) }/config`));

    if (ns) {
      return { clusterId: kept.clusterId, namespace: ns };
    }
  }
  const clusters = await probe('/v1/management.cattle.io.clusters');
  const ids = ((clusters.body as { data?: { id?: unknown }[] } | null)?.data || [])
    .map((c) => c.id)
    .filter((id): id is string => typeof id === 'string' && !!id);
  // Ask every cluster at once; keep the list's order among those that have it.
  const has = await Promise.all(ids.map(async(id) => (await probe(`/k8s/clusters/${ encodeURIComponent(id) }/apis/${ API_GROUP }/${ API_VERSION }`)).status === 200));

  for (const [i, id] of ids.entries()) {
    if (has[i]) {
      const ns = await namespaceIn(id, probe);

      if (ns) {
        return { clusterId: id, namespace: ns };
      }
    }
  }
  throw new BackendNotFound();
}

/** Where the backend is; found once and kept. */
export function findTarget(probe: Probe = fetchProbe): Promise<BackendTarget> {
  if (current) {
    return Promise.resolve(current);
  }
  if (!finding) {
    finding = search(probe).then((t) => {
      remember(t);

      return t;
    }).finally(() => {
      finding = null;
    });
  }

  return finding;
}
