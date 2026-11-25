import { afterEach, describe, expect, it, vi } from 'vitest';
import { call } from './api';

afterEach(() => vi.unstubAllGlobals());

describe('call (typed by the OpenAPI document)', () => {
  it('fills in path placeholders and sends method, body and query', async () => {
    const fetchMock = vi.fn(async (_u: string, _i?: RequestInit) => Response.json({ id: 7 }));
    vi.stubGlobal('fetch', fetchMock);
    await call('post', '/api/admin/users/{id}/password', { params: { id: 7 }, body: { password: 'x'.repeat(8) } });
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('/api/admin/users/7/password');
    expect(init.method).toBe('POST');
    expect(JSON.parse(String(init.body))).toEqual({ password: 'xxxxxxxx' });
    await call('get', '/api/files/list', { query: '?path=a%2Fb' });
    expect(fetchMock.mock.calls[1][0]).toBe('/api/files/list?path=a%2Fb');
    await call('get', '/api/public/{token}', { params: { token: 'a b/c' } });
    expect(fetchMock.mock.calls[2][0]).toBe('/api/public/a%20b%2Fc');
  });

  it('refuses a placeholder with no value', () => {
    expect(() => call('get', '/api/connections/{id}')).toThrow('no value for {id}');
  });
});
