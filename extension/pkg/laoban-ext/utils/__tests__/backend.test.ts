import {
  describe, it, expect, afterEach, jest
} from '@jest/globals';
import {
  parseBackendError, resolveAppUrl, safeHttpsUrl, backendUrl, aiRequestTimeoutMs, ConfigResponse,
  createPlan, getApp, listApps, listVersions, getVersion, restoreVersion, applyPlan,
  runJob, getRound, openRound, roundAction, addOpinions, supportOpinion, replyToOpinion, getFiles, saveFiles, saveLayout, parseKubeStatus
} from '../backend';
import { AI_REQUEST_TIMEOUT_MS } from '../../config/backend';

describe('backendUrl', () => {
  it('goes through Rancher to the aggregated API, under the target namespace', () => {
    expect(backendUrl('/plans')).toBe('/k8s/clusters/c-m-26kbxk5f/apis/api.laoban.yansheng133.github.io/v1alpha1/namespaces/laoban-dev/plans');
  });
});

describe('parseBackendError', () => {
  it('reads {error: {code, message}}', () => {
    expect(parseBackendError({ error: { code: 'budget_exceeded', message: '本月 token 已用完。' } }))
      .toEqual({ code: 'budget_exceeded', message: '本月 token 已用完。' });
  });

  it('ignores bodies that are not the backend error shape', () => {
    expect(parseBackendError(null)).toBeNull();
    expect(parseBackendError('oops')).toBeNull();
    expect(parseBackendError({
      type: 'error', status: 503, message: 'no endpoints available'
    })).toBeNull();
    expect(parseBackendError({ error: { code: 'x', message: '  ' } })).toBeNull();
  });
});

describe('resolveAppUrl', () => {
  const origin = 'https://rancher.example.com';

  it('joins a same-origin path with the Rancher origin', () => {
    expect(resolveAppUrl('/k8s/clusters/c-m-26kbxk5f/api/v1/namespaces/laoban-dev/services/http:breakfast-web:8080/proxy/', origin))
      .toBe('https://rancher.example.com/k8s/clusters/c-m-26kbxk5f/api/v1/namespaces/laoban-dev/services/http:breakfast-web:8080/proxy/');
  });

  it('rejects anything that could leave the origin or run script', () => {
    for (const bad of ['//evil.example.com/x', 'https://evil.example.com/', 'javascript:alert(1)', 'k8s/x', '/\\evil.example.com', '', null, 42]) {
      expect(resolveAppUrl(bad, origin)).toBeNull();
    }
  });
});

describe('safeHttpsUrl', () => {
  it('accepts https only', () => {
    expect(safeHttpsUrl('https://github.com/o/r/commit/abc')).toBe('https://github.com/o/r/commit/abc');
    expect(safeHttpsUrl('http://github.com/o/r/commit/abc')).toBeNull();
    expect(safeHttpsUrl('javascript:alert(1)')).toBeNull();
    expect(safeHttpsUrl(undefined)).toBeNull();
  });
});

describe('aiRequestTimeoutMs', () => {
  const base: ConfigResponse = {
    model: 'm', targetNamespace: 'laoban-dev', repo: 'o/r', tokenBudget: { cap: 1, used: 0 }
  };

  it('waits a minute longer than the backend waits for the AI', () => {
    expect(aiRequestTimeoutMs({ ...base, aiTimeoutSeconds: 900 })).toBe(960 * 1000);
    expect(aiRequestTimeoutMs({ ...base, aiTimeoutSeconds: 300 })).toBe(360 * 1000);
  });

  it('falls back before /api/config has answered or for an older backend', () => {
    expect(aiRequestTimeoutMs(null)).toBe(AI_REQUEST_TIMEOUT_MS);
    expect(aiRequestTimeoutMs(base)).toBe(AI_REQUEST_TIMEOUT_MS);
    expect(aiRequestTimeoutMs({ ...base, aiTimeoutSeconds: 0 })).toBe(AI_REQUEST_TIMEOUT_MS);
  });

  it('caps absurd values at the backend maximum', () => {
    expect(aiRequestTimeoutMs({ ...base, aiTimeoutSeconds: 1e9 })).toBe(1860 * 1000);
  });
});

describe('apps and plan requests', () => {
  const realFetch = globalThis.fetch;
  const store = { getters: {} };
  let calls: { url: string; init: RequestInit }[] = [];

  function mockFetch(status: number, body: unknown) {
    calls = [];
    globalThis.fetch = jest.fn(async(url: unknown, init: unknown) => {
      calls.push({ url: String(url), init: init as RequestInit });

      return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
    }) as unknown as typeof fetch;
  }

  afterEach(() => {
    globalThis.fetch = realFetch;
  });

  it('sends baseApp, not appName, when changing an app', async() => {
    mockFetch(200, { planId: 'p-1', appName: 'todo' });
    await createPlan(store, '加上刪除', 'ignored', 1000, 'todo');
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ prompt: '加上刪除', baseApp: 'todo', background: true });
    expect(calls[0].url).toBe(backendUrl('/plans'));
  });

  it('sends appName for a new app', async() => {
    mockFetch(200, { planId: 'p-1', appName: 'shop' });
    await createPlan(store, 'x', 'shop', 1000);
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ prompt: 'x', appName: 'shop', background: true });
  });

  it('reads the app list and one app', async() => {
    mockFetch(200, { apps: [] });
    expect(await listApps(store)).toEqual({ apps: [] });
    expect(calls[0].url).toBe(backendUrl('/apps'));
    expect(calls[0].init.method).toBe('GET');

    mockFetch(200, { appName: 'a b' });
    await getApp(store, 'a b');
    expect(calls[0].url).toBe(backendUrl('/apps/a%20b'));
  });

  it('surfaces the backend message for app errors', async() => {
    mockFetch(409, { error: { code: 'not_editable', message: '無法編輯。' } });
    await expect(getApp(store, 'old')).rejects.toMatchObject({ code: 'not_editable', userMessage: '無法編輯。', status: 409 });
  });

  it('reads versions and restores with a POST', async() => {
    mockFetch(200, { appName: 'todo', versions: [] });
    await listVersions(store, 'todo');
    expect(calls[0].url).toBe(backendUrl('/apps/todo/versions'));

    const sha = 'a'.repeat(40);

    mockFetch(200, { appName: 'todo', commit: sha });
    await getVersion(store, 'todo', sha);
    expect(calls[0].url).toBe(backendUrl(`/apps/todo/versions/${ sha }`));

    mockFetch(200, { mode: 'restore', commit: 'b'.repeat(40) });
    await restoreVersion(store, 'todo', sha, 1000);
    expect(calls[0].url).toBe(backendUrl('/apps/todo/restore'));
    expect(calls[0].init.method).toBe('POST');
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ commit: sha });
  });

  it('reads and acts on a round of 意見收斂', async() => {
    mockFetch(200, { round: null });
    await getRound(store, 'todo');
    expect(calls[0].url).toBe(backendUrl('/apps/todo/round'));
    expect(calls[0].init.method).toBe('GET');

    mockFetch(200, { round: { state: 'collecting' } });
    await openRound(store, 'todo', [{ text: '要能排序' }, { text: '加搜尋', target: 'app-web' }], 30, 'web', 1000);
    expect(calls[0].init.method).toBe('POST');
    expect(JSON.parse(String(calls[0].init.body))).toEqual({
      items: [{ text: '要能排序' }, { text: '加搜尋', target: 'app-web' }], deadlineMinutes: 30, scope: 'web'
    });

    mockFetch(200, { round: { state: 'collecting' } });
    await addOpinions(store, 'todo', [{ text: 'a', target: 'app-api' }]);
    expect(calls[0].url).toBe(backendUrl('/apps/todo/round/opinions'));
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ items: [{ text: 'a', target: 'app-api' }] });

    mockFetch(200, { round: { state: 'collecting' } });
    await supportOpinion(store, 'todo', 'o3');
    expect(calls[0].url).toBe(backendUrl('/apps/todo/round/opinions/o3/support'));
    mockFetch(200, { round: { state: 'collecting' } });
    await replyToOpinion(store, 'todo', 'o3', '同意');
    expect(calls[0].url).toBe(backendUrl('/apps/todo/round/opinions/o3/replies'));
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ text: '同意' });
    // Only IDs the backend gives out go into the path.
    expect(() => supportOpinion(store, 'todo', '../x')).toThrow();

    mockFetch(200, { round: { state: 'done' }, applied: { commit: 'c' } });
    const res = await roundAction(store, 'todo', 'skip', { user: 'u-b' }, 1000);
    expect(calls[0].url).toBe(backendUrl('/apps/todo/round/skip'));
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ user: 'u-b' });
    expect(res.applied?.commit).toBe('c');
  });

  // Answers in order; the last one repeats.
  function mockFetchSeq(replies: [number, unknown][]) {
    calls = [];
    globalThis.fetch = jest.fn(async(url: unknown, init: unknown) => {
      calls.push({ url: String(url), init: init as RequestInit });
      const [status, body] = replies[Math.min(calls.length - 1, replies.length - 1)];

      return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
    }) as unknown as typeof fetch;
  }

  it('runs a plan as a background job and reads it until done (report 18, B8)', async() => {
    const id = 'j-0123456789abcdef';

    mockFetchSeq([
      [200, { jobId: id, kind: 'plan', state: 'running' }],
      [200, { jobId: id, kind: 'plan', state: 'running' }],
      [200, { jobId: id, kind: 'plan', state: 'done', result: { planId: 'p-1', appName: 'shop' } }],
    ]);
    const plan = await runJob(store, backendUrl('/plans'), { prompt: 'x' }, 5000, 1);

    expect(plan).toEqual({ planId: 'p-1', appName: 'shop' });
    expect(calls.map((c) => c.init.method || 'GET')).toEqual(['POST', 'GET', 'GET']);
    expect(calls[1].url).toBe(backendUrl(`/plans/${ id }`));
  });

  it('a failed job throws the error the request would have thrown', async() => {
    mockFetchSeq([
      [200, { jobId: 'j-0123456789abcdef', state: 'running' }],
      [200, {
        jobId: 'j-0123456789abcdef', state: 'failed', error: { status: 429, code: 'budget_exceeded', message: '已達上限。' }
      }],
    ]);
    await expect(runJob(store, backendUrl('/plans'), {}, 5000, 1)).rejects.toMatchObject({ code: 'budget_exceeded', userMessage: '已達上限。', status: 429 });
  });

  it('keeps reading through a dropped poll, but not past the timeout', async() => {
    mockFetchSeq([
      [200, { jobId: 'j-0123456789abcdef', state: 'running' }],
      [502, 'proxy error'],
      [200, { jobId: 'j-0123456789abcdef', state: 'done', result: { commit: 'c' } }],
    ]);
    expect(await runJob(store, backendUrl('/plans/p-1/apply'), {}, 5000, 1)).toEqual({ commit: 'c' });

    mockFetchSeq([[200, { jobId: 'j-0123456789abcdef', state: 'running' }]]);
    await expect(runJob(store, backendUrl('/plans'), {}, 30, 5)).rejects.toMatchObject({ code: 'timeout' });
  });

  it('takes a direct answer from a backend without jobs', async() => {
    mockFetch(200, { commit: 'c' });
    expect(await applyPlan(store, 'p-20261013-abcdef', 1000)).toEqual({ commit: 'c' });
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ background: true });
  });

  it('applies a plan by its ID in the path', async() => {
    mockFetch(200, { commit: 'c' });
    await applyPlan(store, 'p-20261013-abcdef', 1000);
    expect(calls[0].url).toBe(backendUrl('/plans/p-20261013-abcdef/apply'));
    expect(calls[0].init.method).toBe('POST');
  });

  it("a backend error that is also a Kubernetes Status keeps the backend's message", async() => {
    // The backend's errors carry kind: Status so kubectl prints them; the
    // page must not take a 404 or 403 from the backend for the API
    // server's "not reachable" or "no permission".
    for (const [status, code] of [[404, 'job_not_found'], [403, 'not_initiator'], [409, 'round_state']] as [number, string][]) {
      mockFetch(status, {
        kind: 'Status', apiVersion: 'v1', status: 'Failure', message: '後端的訊息。', reason: 'NotFound', code: status, error: { code, message: '後端的訊息。' }
      });
      await expect(getApp(store, 'todo')).rejects.toMatchObject({ code, userMessage: '後端的訊息。', status });
    }
  });

  it('turns API server errors into messages', async() => {
    expect(parseKubeStatus({ kind: 'Status', code: 403 }, 403)?.code).toBe('forbidden');
    expect(parseKubeStatus({ kind: 'Status', code: 503 }, 503)?.code).toBe('unavailable');
    expect(parseKubeStatus({ error: {} }, 403)).toBeNull();
    mockFetch(403, { kind: 'Status', code: 403, message: 'forbidden: User "u-x" cannot create resource' });
    await expect(getApp(store, 'todo')).rejects.toMatchObject({ code: 'forbidden', status: 403 });
  });

  it('reads and saves code and layout', async() => {
    const files = {
      web: {
        'index.html': 'a', 'app.js': 'b', 'style.css': 'c'
      },
      api: { 'handler.py': 'def handle(m, p, b):\n    return 200, {}\n' },
    };

    mockFetch(200, { appName: 'todo', commit: 'c', files });
    await getFiles(store, 'todo');
    expect(calls[0].url).toBe(backendUrl('/apps/todo/files'));

    mockFetch(200, { mode: 'manual', commit: 'd' });
    await saveFiles(store, 'todo', 'c', files, '', 1000);
    expect(calls[0].init.method).toBe('POST');
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ baseCommit: 'c', files });

    mockFetch(200, { mode: 'layout', commit: 'e' });
    await saveLayout(store, 'todo', 'c', { 'mod-web-x': { x: 1, z: 2 } }, 1000);
    expect(calls[0].url).toBe(backendUrl('/apps/todo/layout'));
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ baseCommit: 'c', positions: { 'mod-web-x': { x: 1, z: 2 } } });
  });
});
