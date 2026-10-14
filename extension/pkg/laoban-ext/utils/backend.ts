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
  mode?: 'create' | 'edit' | 'restore' | 'manual' | 'layout';
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
  // Rancher user ID verified by the API server; absent in older entries.
  by?: string;
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
  lock?: LockStatus | null;
}

// Who is editing an app (GET /api/apps/<name>, "lock").
export interface LockStatus {
  holder: string;
  since: string;
}

// POST .../apps/<name>/lock
export interface LockGrant {
  holder: string;
  ttlSeconds: number;
}

// The four generated files, as the AI and the editor see them.
export interface AppFiles {
  web: { 'index.html': string; 'app.js': string; 'style.css': string };
  api: { 'handler.py': string };
}

// GET /api/apps/<name>/files
export interface AppFilesResponse {
  appName: string;
  commit: string;
  files: AppFiles;
}

export interface Position { x: number; z: number }

// One entry of GET /api/apps/<name>/versions (newest first).
export interface VersionEntry {
  version: number;
  commit: string;
  commitUrl: string;
  at: string;
  kind: 'create' | 'edit' | 'restore' | 'manual' | 'layout' | 'commit' | string;
  request: string;
  summary: string;
  model: string;
  // Rancher user ID verified by the API server; "" in older versions.
  by?: string;
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
 * A Kubernetes Status from the API server in front of the backend (the
 * backend is an aggregated API): no permission, or the backend not
 * reachable. Mapped to the page's own codes; the message is shown as is.
 */
export function parseKubeStatus(body: unknown, status: number): { code: string; message: string } | null {
  if (!body || typeof body !== 'object' || (body as { kind?: unknown }).kind !== 'Status') {
    return null;
  }
  if (status === 403) {
    return { code: 'forbidden', message: '你沒有這個操作的權限：需要部署目標 namespace（Rancher Project）的編輯權限，請洽管理者。' };
  }
  if (status === 401) {
    return { code: 'unauthenticated', message: '登入已失效，請重新登入 Rancher。' };
  }
  if (status === 503 || status === 404) {
    return { code: 'unavailable', message: 'laoban 後端目前無法連線（APIService 尚未就緒），請稍後再試或請管理者查看。' };
  }

  return null;
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
  // Let the request finish while the page unloads (releasing the lock).
  keepalive?: boolean;
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
      keepalive:   opts.keepalive || undefined,
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
    const err = parseBackendError(data) || parseKubeStatus(data, res.status);

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
  return requestJson<ConfigResponse>(store, backendUrl('/config'));
}

export function listApps(store: StoreLike): Promise<{ apps: AppSummary[] }> {
  return requestJson<{ apps: AppSummary[] }>(store, backendUrl('/apps'));
}

export function getApp(store: StoreLike, name: string): Promise<AppDetail> {
  return requestJson<AppDetail>(store, backendUrl(`/apps/${ encodeURIComponent(name) }`));
}

export function listVersions(store: StoreLike, name: string): Promise<{ appName: string; versions: VersionEntry[]; truncated?: boolean }> {
  return requestJson(store, backendUrl(`/apps/${ encodeURIComponent(name) }/versions`));
}

export function getVersion(store: StoreLike, name: string, commit: string): Promise<VersionDetail> {
  return requestJson<VersionDetail>(store, backendUrl(`/apps/${ encodeURIComponent(name) }/versions/${ encodeURIComponent(commit) }`));
}

function appPath(name: string, rest = ''): string {
  return backendUrl(`/apps/${ encodeURIComponent(name) }${ rest }`);
}

/** Publish an earlier version again as a new version (no AI call). */
export function restoreVersion(store: StoreLike, name: string, commit: string, timeoutMs: number): Promise<ApplyResponse> {
  return requestJson<ApplyResponse>(store, appPath(name, '/restore'), {
    method: 'POST', body: { commit }, timeoutMs
  });
}

// The lock holder is the signed-in user as verified by the API server.
export function acquireLock(store: StoreLike, name: string): Promise<LockGrant> {
  return requestJson<LockGrant>(store, appPath(name, '/lock'), { method: 'POST', body: {} });
}

export function renewLock(store: StoreLike, name: string): Promise<{ ttlSeconds: number }> {
  return requestJson(store, appPath(name, '/lock/renew'), { method: 'POST', body: {} });
}

export function releaseLock(store: StoreLike, name: string): Promise<{ released: boolean }> {
  return requestJson(store, appPath(name, '/unlock'), {
    method: 'POST', body: {}, keepalive: true
  });
}

export function getFiles(store: StoreLike, name: string): Promise<AppFilesResponse> {
  return requestJson<AppFilesResponse>(store, appPath(name, '/files'));
}

/** Hand-edited code as a new version (no AI call). */
export function saveFiles(store: StoreLike, name: string, baseCommit: string, files: AppFiles, note: string, timeoutMs: number): Promise<ApplyResponse> {
  return requestJson<ApplyResponse>(store, appPath(name, '/files'), {
    method: 'POST', body: { baseCommit, files, note: note || undefined }, timeoutMs
  });
}

/** Dragged 3D positions as a new version (no AI call). */
export function saveLayout(store: StoreLike, name: string, baseCommit: string, positions: Record<string, Position>, timeoutMs: number): Promise<ApplyResponse> {
  return requestJson<ApplyResponse>(store, appPath(name, '/layout'), {
    method: 'POST', body: { baseCommit, positions }, timeoutMs
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

  return requestJson<PlanResponse>(store, backendUrl('/plans'), {
    method: 'POST', body, timeoutMs
  });
}

export function applyPlan(store: StoreLike, planId: string, timeoutMs: number): Promise<ApplyResponse> {
  return requestJson<ApplyResponse>(store, backendUrl(`/plans/${ encodeURIComponent(planId) }/apply`), {
    method: 'POST', body: {}, timeoutMs
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
