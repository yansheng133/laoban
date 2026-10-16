// HTTP client for the laoban backend (through the Rancher service proxy) and
// for Fleet bundles (through Steve). Both use the user's Rancher session:
// the browser sends the session cookie, and state-changing requests carry the
// CSRF token the same way Rancher's own axios plugin does.

import {
  AI_REQUEST_MARGIN_MS, AI_REQUEST_TIMEOUT_MS, API_GROUP, FLEET_BUNDLES_PATH, DEFAULT_REQUEST_TIMEOUT_MS
} from '../config/backend';
import { apiBase, findTarget, BackendNotFound } from './target';
import type { Architecture } from './layout';
import type { RoundLimits } from './limits';
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
  // State of a running round of 意見收斂, if any.
  round?: RoundState | null;
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
  fileDigests?: Record<string, string>;
  updatedAt: string;
  history: AppHistoryEntry[];
  appUrl: string;
  path: string;
  fleet: FleetRef;
  // The latest round of 意見收斂, as seen by the caller.
  round?: RoundView | null;
}

export type RoundState = 'collecting' | 'converging' | 'confirming' | 'applying' | 'done' | 'failed' | 'cancelled';

export type RoundPart = 'web' | 'api';

// A round of 意見收斂 (GET .../apps/<name>/round). Users are the IDs the
// API server verified; `you` is what the caller may do.
export interface RoundView {
  id: string;
  app: string;
  initiator: string;
  scope: RoundPart[];
  state: RoundState;
  // Unix seconds; when collecting ends.
  deadline: number;
  // Unix seconds: the deadline can be moved up to this (24 h after opening).
  latestDeadline?: number;
  opinions: RoundOpinion[];
  confirmers: { user: string; answer: 'pending' | 'confirmed' | 'skipped'; at?: string }[];
  plan: { planId: string; summary: string; steps: string[]; architecture: Architecture; notes?: string } | null;
  message: string;
  // The failure as a code the page translates (laoban.round.failures.*).
  messageCode?: string;
  messageArgs?: Record<string, string>;
  result: ApplyResponse | null;
  log: { at: string; by: string; event: string; code?: string; args?: Record<string, string> }[];
  you: { user: string; isInitiator: boolean; mustConfirm: boolean; canAddOpinion: boolean; canDiscuss?: boolean; canComment?: boolean };
  // Size limits and what the round has used of them (the backend counts).
  limits?: RoundLimits;
  used?: { chars: number; opinions: number };
  // The caller's own drafts, and the ones others share.
  drafts?: { mine: RoundDrafts | null; shared: { user: string; items: RoundDraft[] }[] };
  // Comments on the steps of the converged plan (while confirming).
  planComments?: PlanComment[];
}

export interface RoundDraft {
  target?: { id: string; name: string };
  text: string;
  at: string;
}

export interface RoundDrafts {
  shared: boolean;
  items: RoundDraft[];
}

export interface PlanComment {
  id: string;
  // 1-based.
  step: number;
  stepText: string;
  by: string;
  at: string;
  text: string;
  supporters: string[];
}

// A round waiting for the caller (GET .../waitingrounds).
export interface WaitingRound {
  app: string;
  roundId: string;
  state: RoundState;
  // "confirm": the caller has to confirm; "othersConfirm": the caller
  // started it or answered, and others have not; "retry": it failed and the
  // caller started it; "failed": it failed after the caller confirmed.
  reason: 'confirm' | 'othersConfirm' | 'retry' | 'failed';
  pending: number;
}

// An archived round as listed, and as read.
export interface PastRoundEntry {
  id: string;
  // YYYY-MM-DD
  date: string;
}

export interface PastRound {
  round: RoundView;
  archivedAt?: string;
  openedAt?: string;
}

// One opinion: optionally about one component of the architecture, with
// the people who +1'd it and the replies under it.
export interface RoundOpinion {
  id: string;
  by: string;
  at: string;
  text: string;
  target?: { id: string; name: string };
  supporters?: string[];
  replies?: { by: string; at: string; text: string }[];
  // Set when the author changed it; editedAfterSupport when +1s came first.
  editedAt?: string;
  editedAfterSupport?: boolean;
  // Taken back by the author: the text is gone, the number stays.
  withdrawn?: boolean;
}

// An opinion as sent; `target` is the architecture node id.
export interface OpinionItem {
  text: string;
  target?: string;
}

export interface RoundResponse {
  round: RoundView | null;
  // Present when this call published the change.
  applied?: ApplyResponse;
  // False when the backend cannot store rounds (the Round CRD is missing).
  roundsAvailable?: boolean;
  // Limits for a new round.
  limits?: RoundLimits;
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
  // Path → short hash of each code file (v0.1.14 and later).
  fileDigests?: Record<string, string>;
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

  const target = await resolveBackend(url);
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), opts.timeoutMs || DEFAULT_REQUEST_TIMEOUT_MS);
  let res: Response;

  try {
    res = await fetch(target, {
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

// Backend paths are written as BACKEND + path; the cluster and namespace
// are put in when the request is made (utils/target.ts).
export const BACKEND = 'laoban-backend:';

export function backendUrl(path: string): string {
  return `${ BACKEND }${ path }`;
}

// Whether the signed-in user may change apps (create in laoban's API), as
// the API server decides: a SelfSubjectAccessReview, which anyone may ask.
// Read once; on any failure the page assumes yes and the backend decides.
let mayChange: Promise<boolean> | null = null;

export function canChangeApps(store: StoreLike): Promise<boolean> {
  if (!mayChange) {
    mayChange = (async() => {
      try {
        const target = await findTarget();
        const res = await requestJson<{ status?: { allowed?: boolean } }>(store,
          `/k8s/clusters/${ encodeURIComponent(target.clusterId) }/apis/authorization.k8s.io/v1/selfsubjectaccessreviews`, {
            method: 'POST',
            body:   {
              apiVersion: 'authorization.k8s.io/v1',
              kind:       'SelfSubjectAccessReview',
              spec:       {
                resourceAttributes: {
                  namespace: target.namespace, verb: 'create', group: API_GROUP, resource: 'apps', subresource: 'files'
                }
              }
            }
          });

        return res.status?.allowed !== false;
      } catch {
        return true;
      }
    })();
  }

  return mayChange;
}

/** Forget the answer (tests). */
export function resetAccess() {
  mayChange = null;
}

async function resolveBackend(url: string): Promise<string> {
  if (!url.startsWith(BACKEND)) {
    return url;
  }
  try {
    return `${ apiBase(await findTarget()) }${ url.slice(BACKEND.length) }`;
  } catch (e) {
    if (e instanceof BackendNotFound) {
      throw new RequestError('backend_not_found', null, 404);
    }
    throw new RequestError('network', null);
  }
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

export function getRound(store: StoreLike, name: string): Promise<RoundResponse> {
  return requestJson<RoundResponse>(store, appPath(name, '/round'));
}

/** Open a round. With deadlineMinutes 0 the AI converges in this call. */
export function openRound(store: StoreLike, name: string, items: OpinionItem[], deadlineMinutes: number, scope: string, timeoutMs: number): Promise<RoundResponse> {
  return requestJson<RoundResponse>(store, appPath(name, '/round'), {
    method: 'POST', body: { items, deadlineMinutes, scope }, timeoutMs
  });
}

/** Several opinions at once (up to 10), each optionally about a component. */
export function addOpinions(store: StoreLike, name: string, items: OpinionItem[]): Promise<RoundResponse> {
  return requestJson<RoundResponse>(store, appPath(name, '/round/opinions'), { method: 'POST', body: { items } });
}

const OPINION_ID = /^o[0-9]{1,4}$/;

function opinionPath(name: string, id: string, what: string): string {
  if (!OPINION_ID.test(id)) {
    throw new RequestError('bad_request', null);
  }

  return appPath(name, `/round/opinions/${ id }/${ what }`);
}

/** +1 an opinion, or take the +1 back. */
export function supportOpinion(store: StoreLike, name: string, id: string): Promise<RoundResponse> {
  return requestJson<RoundResponse>(store, opinionPath(name, id, 'support'), { method: 'POST', body: {} });
}

export function replyToOpinion(store: StoreLike, name: string, id: string, text: string): Promise<RoundResponse> {
  return requestJson<RoundResponse>(store, opinionPath(name, id, 'replies'), { method: 'POST', body: { text } });
}

/** The author changes an opinion while collecting. */
export function editOpinion(store: StoreLike, name: string, id: string, text: string): Promise<RoundResponse> {
  return requestJson<RoundResponse>(store, opinionPath(name, id, 'edit'), { method: 'POST', body: { text } });
}

/** The author takes an opinion back while collecting. */
export function withdrawOpinion(store: StoreLike, name: string, id: string): Promise<RoundResponse> {
  return requestJson<RoundResponse>(store, opinionPath(name, id, 'withdraw'), { method: 'POST', body: {} });
}

/** The initiator moves the deadline (minutes from now; 0 converges now). */
export function setRoundDeadline(store: StoreLike, name: string, minutes: number, timeoutMs?: number): Promise<RoundResponse> {
  return requestJson<RoundResponse>(store, appPath(name, '/round/deadline'), { method: 'POST', body: { minutes }, timeoutMs });
}

/** Keep the caller's drafts in the round (replacing the ones kept). */
export function saveRoundDrafts(store: StoreLike, name: string, items: OpinionItem[], shared: boolean): Promise<RoundResponse> {
  return requestJson<RoundResponse>(store, appPath(name, '/round/drafts'), { method: 'POST', body: { items, shared } });
}

/** Comment on step `step` (1-based) of the converged plan. */
export function commentOnPlan(store: StoreLike, name: string, step: number, text: string): Promise<RoundResponse> {
  if (!Number.isInteger(step) || step < 1 || step > 100) {
    throw new RequestError('bad_request', null);
  }

  return requestJson<RoundResponse>(store, appPath(name, `/round/plan/steps/${ step }/comments`), { method: 'POST', body: { text } });
}

const COMMENT_ID = /^c[1-9][0-9]{0,4}$/;

/** +1 a comment on the plan, or take it back. */
export function supportPlanComment(store: StoreLike, name: string, id: string): Promise<RoundResponse> {
  if (!COMMENT_ID.test(id)) {
    throw new RequestError('bad_request', null);
  }

  return requestJson<RoundResponse>(store, appPath(name, `/round/plan/comments/${ id }/support`), { method: 'POST', body: {} });
}

/** Rounds on any app that wait for the caller. */
export function getWaitingRounds(store: StoreLike): Promise<{ rounds: WaitingRound[]; roundsAvailable?: boolean }> {
  return requestJson(store, backendUrl('/waitingrounds'));
}

/** The app's archived rounds, newest first. */
export function listPastRounds(store: StoreLike, name: string): Promise<{ rounds: PastRoundEntry[]; truncated?: boolean }> {
  return requestJson(store, appPath(name, '/rounds'));
}

const ROUND_ID = /^r-[0-9]{8}-[0-9a-f]{6}$/;

export function getPastRound(store: StoreLike, name: string, id: string): Promise<PastRound> {
  if (!ROUND_ID.test(id)) {
    throw new RequestError('bad_request', null);
  }

  return requestJson(store, appPath(name, `/rounds/${ id }`));
}

export type RoundAction = 'opinions' | 'confirm' | 'object' | 'skip' | 'cancel' | 'converge';

/**
 * One action on the current round. "confirm"/"skip" may publish (the AI
 * writes the code) and "converge" runs the AI, so they get `timeoutMs`.
 */
export function roundAction(store: StoreLike, name: string, action: RoundAction, body: Record<string, string>, timeoutMs?: number): Promise<RoundResponse> {
  return requestJson<RoundResponse>(store, appPath(name, `/round/${ action }`), {
    method: 'POST', body, timeoutMs
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

  return runJob<PlanResponse>(store, backendUrl('/plans'), body, timeoutMs);
}

export function applyPlan(store: StoreLike, planId: string, timeoutMs: number): Promise<ApplyResponse> {
  return runJob<ApplyResponse>(store, backendUrl(`/plans/${ encodeURIComponent(planId) }/apply`), {}, timeoutMs);
}

// A background plan or apply (GET .../plans/<jobId>).
export interface JobView {
  jobId: string;
  kind: string;
  state: 'running' | 'done' | 'failed';
  elapsedSeconds?: number;
  result?: unknown;
  error?: { status?: number; code?: string; message?: string };
}

export const JOB_POLL_MS = 2000;

const JOB_ID = /^j-[0-9a-f]{16}$/;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * POST a request that runs the AI as a background job, then read the job
 * until it is done (report 18, B8: the Kubernetes API server ends requests
 * after about 60 s). Resolves with the result, or throws the error the
 * request itself would have thrown. A backend that answers with the result
 * directly (no job) is taken as is.
 */
export async function runJob<T>(store: StoreLike, url: string, body: Record<string, unknown>, timeoutMs: number, pollMs = JOB_POLL_MS): Promise<T> {
  const started = Date.now();
  const first = await requestJson<T | JobView>(store, url, { method: 'POST', body: { ...body, background: true } });
  const job = first as JobView;

  if (!job || typeof job !== 'object' || typeof job.jobId !== 'string') {
    return first as T;
  }
  if (!JOB_ID.test(job.jobId)) {
    throw new RequestError('bad_response', null);
  }
  let view: JobView = job;

  while (view.state === 'running') {
    if (Date.now() - started > timeoutMs) {
      throw new RequestError('timeout', null);
    }
    await sleep(pollMs);
    try {
      view = await requestJson<JobView>(store, backendUrl(`/plans/${ job.jobId }`));
    } catch (e) {
      // A dropped poll is retried; the backend's own answer (the job is
      // gone, no permission) is final.
      if (e instanceof RequestError && e.userMessage) {
        throw e;
      }
    }
  }
  if (view.state === 'failed') {
    const err = view.error || {};

    throw new RequestError(err.code || 'unknown', err.message || null, err.status || 0);
  }

  return view.result as T;
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
