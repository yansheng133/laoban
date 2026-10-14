// HTTP client for the laoban backend (through the Rancher service proxy) and
// for Fleet bundles (through Steve). Both use the user's Rancher session:
// the browser sends the session cookie, and state-changing requests carry the
// CSRF token the same way Rancher's own axios plugin does.

import {
  AI_REQUEST_MARGIN_MS, AI_REQUEST_TIMEOUT_MS, BACKEND_BASE_PATH, FLEET_BUNDLES_PATH, DEFAULT_REQUEST_TIMEOUT_MS
} from '../config/backend';
import type { Architecture } from './layout';
import type { Deployment } from './rollout';
import type { Bundle } from './bundle';

// The part of the Vuex store this module uses (the shell's cookies getter).
export interface StoreLike {
  getters?: Record<string, unknown>;
}

export interface PlanResponse {
  planId: string;
  // 'edit' when the plan changes an existing app (baseApp).
  mode?: 'create' | 'edit';
  appName: string;
  summary: string;
  steps: string[];
  architecture: Architecture;
  maskedFindings?: number;
  usage?: { inputTokens?: number; outputTokens?: number };
}

export interface ApplyResponse {
  mode?: 'create' | 'edit' | 'restore';
  appName?: string;
  // Deployments to check after Fleet is ready (see utils/rollout.ts).
  rollout?: unknown;
  commit: string;
  commitUrl: string;
  path: string;
  appUrl: string;
  fleet: { workspace: string; gitRepo: string; bundlePrefix: string };
}

export interface FleetRef { workspace: string; gitRepo: string; bundlePrefix: string }

// One entry of GET /api/apps. `editable` is false for apps without a
// laoban.json (made before it existed); those have no summary or date.
export interface AppSummary {
  appName: string;
  editable: boolean;
  summary: string;
  updatedAt: string;
  revisions: number;
  appUrl: string;
  path: string;
  fleet: FleetRef;
}

export interface AppHistoryEntry {
  at: string;
  kind: 'create' | 'edit' | string;
  request: string;
  summary: string;
  model: string;
}

// GET /api/apps/<name>
export interface AppDetail {
  appName: string;
  summary: string;
  steps: string[];
  architecture: Architecture;
  updatedAt: string;
  history: AppHistoryEntry[];
  appUrl: string;
  path: string;
  fleet: FleetRef;
}

// One entry of GET /api/apps/<name>/versions (newest first).
export interface VersionEntry {
  version: number;
  commit: string;
  commitUrl: string;
  at: string;
  kind: 'create' | 'edit' | 'restore' | 'commit' | string;
  request: string;
  summary: string;
  model: string;
  current: boolean;
}

// GET /api/apps/<name>/versions/<commit>
export interface VersionDetail {
  appName: string;
  commit: string;
  summary: string;
  steps: string[];
  architecture: Architecture;
  updatedAt: string;
}

export interface ConfigResponse {
  model: string;
  targetNamespace: string;
  repo: string;
  tokenBudget: { cap: number; used: number };
  // Seconds the backend waits for one AI call (LAOBAN_AI_TIMEOUT_SECONDS).
  aiTimeoutSeconds?: number;
}

// How long the page waits for /api/plan and /api/apply: a little longer than
// the backend waits for the AI, so the backend's own error message (not a
// generic timeout) is what the user sees.
export function aiRequestTimeoutMs(config: ConfigResponse | null | undefined): number {
  const secs = config?.aiTimeoutSeconds;

  if (typeof secs !== 'number' || !Number.isFinite(secs) || secs <= 0) {
    return AI_REQUEST_TIMEOUT_MS;
  }

  return Math.min(secs, 1800) * 1000 + AI_REQUEST_MARGIN_MS;
}

// `message` is the backend's user-facing (Traditional Chinese) sentence, or
// null when there is none to show (network failure, proxy error, bad JSON);
// the page then shows its own generic text.
export class RequestError extends Error {
  code: string;
  userMessage: string | null;
  status: number;

  constructor(code: string, userMessage: string | null, status = 0) {
    super(userMessage || code);
    this.name = 'RequestError';
    this.code = code;
    this.userMessage = userMessage;
    this.status = status;
  }
}

/**
 * Extract `{error: {code, message}}` from a backend error body. Anything else
 * (for example a Rancher proxy error) returns null.
 */
export function parseBackendError(body: unknown): { code: string; message: string } | null {
  if (!body || typeof body !== 'object') {
    return null;
  }
  const err = (body as { error?: unknown }).error as { code?: unknown; message?: unknown } | null | undefined;

  if (!err || typeof err !== 'object' || typeof err.message !== 'string' || !err.message.trim()) {
    return null;
  }

  return { code: typeof err.code === 'string' ? err.code : 'unknown', message: err.message };
}

const CSRF_COOKIE = 'CSRF';

function csrfToken(store: StoreLike): string | undefined {
  try {
    const get = store?.getters?.['cookies/get'] as ((arg: { key: string; options: { parseJSON: boolean } }) => unknown) | undefined;
    const v = get?.({ key: CSRF_COOKIE, options: { parseJSON: false } });

    return typeof v === 'string' && v ? v : undefined;
  } catch {
    return undefined;
  }
}

interface RequestOptions {
  method?: 'GET' | 'POST';
  body?: unknown;
  timeoutMs?: number;
}

async function requestJson<T>(store: StoreLike, url: string, opts: RequestOptions = {}): Promise<T> {
  const method = opts.method || 'GET';
  const headers: Record<string, string> = { Accept: 'application/json' };

  if (opts.body !== undefined) {
    headers['Content-Type'] = 'application/json';
  }
  if (method !== 'GET') {
    const csrf = csrfToken(store);

    if (csrf) {
      headers['X-Api-Csrf'] = csrf;
    }
  }

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), opts.timeoutMs || DEFAULT_REQUEST_TIMEOUT_MS);
  let res: Response;

  try {
    res = await fetch(url, {
      method,
      headers,
      credentials: 'same-origin',
      body:        opts.body === undefined ? undefined : JSON.stringify(opts.body),
      signal:      controller.signal,
    });
  } catch (e) {
    throw new RequestError(controller.signal.aborted ? 'timeout' : 'network', null);
  } finally {
    clearTimeout(timer);
  }

  let data: unknown = null;
  let parsed = true;

  try {
    data = await res.json();
  } catch {
    parsed = false;
  }

  if (!res.ok) {
    const err = parseBackendError(data);

    if (err) {
      throw new RequestError(err.code, err.message, res.status);
    }
    throw new RequestError('http', null, res.status);
  }
  if (!parsed || data === null || typeof data !== 'object') {
    throw new RequestError('invalid_response', null, res.status);
  }

  return data as T;
}

export function backendUrl(path: string): string {
  return `${ BACKEND_BASE_PATH }${ path }`;
}

export function getConfig(store: StoreLike): Promise<ConfigResponse> {
  return requestJson<ConfigResponse>(store, backendUrl('/api/config'));
}

export function listApps(store: StoreLike): Promise<{ apps: AppSummary[] }> {
  return requestJson<{ apps: AppSummary[] }>(store, backendUrl('/api/apps'));
}

export function getApp(store: StoreLike, name: string): Promise<AppDetail> {
  return requestJson<AppDetail>(store, backendUrl(`/api/apps/${ encodeURIComponent(name) }`));
}

export function listVersions(store: StoreLike, name: string): Promise<{ appName: string; versions: VersionEntry[]; truncated?: boolean }> {
  return requestJson(store, backendUrl(`/api/apps/${ encodeURIComponent(name) }/versions`));
}

export function getVersion(store: StoreLike, name: string, commit: string): Promise<VersionDetail> {
  return requestJson<VersionDetail>(store, backendUrl(`/api/apps/${ encodeURIComponent(name) }/versions/${ encodeURIComponent(commit) }`));
}

/** Publish an earlier version again as a new version (no AI call). */
export function restoreVersion(store: StoreLike, name: string, commit: string, timeoutMs: number): Promise<ApplyResponse> {
  return requestJson<ApplyResponse>(store, backendUrl(`/api/apps/${ encodeURIComponent(name) }/restore`), {
    method: 'POST', body: { commit }, timeoutMs
  });
}

/**
 * New app: `appName` optional. Change an existing app: pass `baseApp`
 * (the backend keeps its name).
 */
export function createPlan(store: StoreLike, prompt: string, appName: string | undefined, timeoutMs: number, baseApp?: string): Promise<PlanResponse> {
  const body: Record<string, string> = { prompt };

  if (baseApp) {
    body.baseApp = baseApp;
  } else if (appName) {
    body.appName = appName;
  }

  return requestJson<PlanResponse>(store, backendUrl('/api/plan'), {
    method: 'POST', body, timeoutMs
  });
}

export function applyPlan(store: StoreLike, planId: string, timeoutMs: number): Promise<ApplyResponse> {
  return requestJson<ApplyResponse>(store, backendUrl('/api/apply'), {
    method: 'POST', body: { planId }, timeoutMs
  });
}

/**
 * One Deployment in the target cluster through Steve, or null if it does not
 * exist yet. `path` comes from rolloutTargets(), which only allows Steve
 * deployment paths.
 */
export async function getDeployment(store: StoreLike, path: string): Promise<Deployment | null> {
  try {
    return await requestJson<Deployment>(store, path);
  } catch (e) {
    if (e instanceof RequestError && e.status === 404) {
      return null;
    }
    throw e;
  }
}

/** List Fleet bundles in the workspace through Steve on the local cluster. */
export async function listBundles(store: StoreLike): Promise<Bundle[]> {
  const res = await requestJson<{ data?: Bundle[] }>(store, FLEET_BUNDLES_PATH);

  return Array.isArray(res.data) ? res.data : [];
}

/**
 * Turn the backend's appUrl into an absolute URL on the Rancher origin.
 * Only same-origin absolute paths ("/k8s/...") are accepted; anything else
 * (another host, "//host", "javascript:") returns null.
 */
export function resolveAppUrl(appUrl: unknown, origin: string): string | null {
  if (typeof appUrl !== 'string' || !appUrl.startsWith('/') || appUrl.startsWith('//') || appUrl.includes('\\')) {
    return null;
  }
  try {
    const u = new URL(appUrl, origin);

    return u.origin === new URL(origin).origin ? u.toString() : null;
  } catch {
    return null;
  }
}

/** Accept only https links for the commit (the backend points at GitHub). */
export function safeHttpsUrl(url: unknown): string | null {
  if (typeof url !== 'string') {
    return null;
  }
  try {
    const u = new URL(url);

    return u.protocol === 'https:' ? u.toString() : null;
  } catch {
    return null;
  }
}
