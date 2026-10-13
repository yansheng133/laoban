// HTTP client for the laoban backend (through the Rancher service proxy) and
// for Fleet bundles (through Steve). Both use the user's Rancher session:
// the browser sends the session cookie, and state-changing requests carry the
// CSRF token the same way Rancher's own axios plugin does.

import { BACKEND_BASE_PATH, FLEET_BUNDLES_PATH, DEFAULT_REQUEST_TIMEOUT_MS } from '../config/backend';
import type { Architecture } from './layout';
import type { Bundle } from './bundle';

// The part of the Vuex store this module uses (the shell's cookies getter).
export interface StoreLike {
  getters?: Record<string, unknown>;
}

export interface PlanResponse {
  planId: string;
  appName: string;
  summary: string;
  steps: string[];
  architecture: Architecture;
  maskedFindings?: number;
  usage?: { inputTokens?: number; outputTokens?: number };
}

export interface ApplyResponse {
  commit: string;
  commitUrl: string;
  path: string;
  appUrl: string;
  fleet: { workspace: string; gitRepo: string; bundlePrefix: string };
}

export interface ConfigResponse {
  model: string;
  targetNamespace: string;
  repo: string;
  tokenBudget: { cap: number; used: number };
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

export function createPlan(store: StoreLike, prompt: string, appName: string | undefined, timeoutMs: number): Promise<PlanResponse> {
  const body: Record<string, string> = { prompt };

  if (appName) {
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
