import {
  describe, it, expect, afterEach, jest
} from '@jest/globals';
import {
  parseBackendError, resolveAppUrl, safeHttpsUrl, backendUrl, aiRequestTimeoutMs, ConfigResponse,
  createPlan, getApp, listApps
} from '../backend';
import { AI_REQUEST_TIMEOUT_MS } from '../../config/backend';

describe('backendUrl', () => {
  it('goes through the Rancher service proxy', () => {
    expect(backendUrl('/api/plan')).toBe('/k8s/clusters/c-m-26kbxk5f/api/v1/namespaces/laoban-system/services/http:laoban:8080/proxy/api/plan');
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
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ prompt: '加上刪除', baseApp: 'todo' });
    expect(calls[0].url).toBe(backendUrl('/api/plan'));
  });

  it('sends appName for a new app', async() => {
    mockFetch(200, { planId: 'p-1', appName: 'shop' });
    await createPlan(store, 'x', 'shop', 1000);
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ prompt: 'x', appName: 'shop' });
  });

  it('reads the app list and one app', async() => {
    mockFetch(200, { apps: [] });
    expect(await listApps(store)).toEqual({ apps: [] });
    expect(calls[0].url).toBe(backendUrl('/api/apps'));
    expect(calls[0].init.method).toBe('GET');

    mockFetch(200, { appName: 'a b' });
    await getApp(store, 'a b');
    expect(calls[0].url).toBe(backendUrl('/api/apps/a%20b'));
  });

  it('surfaces the backend message for app errors', async() => {
    mockFetch(409, { error: { code: 'not_editable', message: '無法編輯。' } });
    await expect(getApp(store, 'old')).rejects.toMatchObject({ code: 'not_editable', userMessage: '無法編輯。', status: 409 });
  });
});
