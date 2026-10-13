import { describe, it, expect } from '@jest/globals';
import {
  parseBackendError, resolveAppUrl, safeHttpsUrl, backendUrl, aiRequestTimeoutMs, ConfigResponse
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
