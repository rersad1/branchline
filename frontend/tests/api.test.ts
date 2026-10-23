import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiClient, worktreeCacheKey } from '../src/api';
afterEach(() => vi.unstubAllGlobals());
describe('Rust API client', () => {
  it('scopes identical project/path identities to distinct environments and rejects foreign snapshots', async () => {
    expect(worktreeCacheKey('Ubuntu', 'same-id')).not.toEqual(worktreeCacheKey('Ubuntu-2', 'same-id'));
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({ environment_id: 'Ubuntu-2', project_id: 'same-id', worktrees: [] })));
    vi.stubGlobal('fetch', fetcher);
    await expect(new ApiClient('memory-only').worktrees('same-id', undefined, 'Ubuntu')).rejects.toMatchObject({ kind: 'error' });
    expect(fetcher.mock.calls[0][0]).toBe('/api/v1/projects/same-id/worktrees?environment_id=Ubuntu');
  });
  it('uses only a relative Rust route, memory token and no browser credentials', async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response('[]'));
    vi.stubGlobal('fetch', fetcher);
    await new ApiClient('runtime-only').projects();
    expect(fetcher).toHaveBeenCalledWith('/api/v1/projects', expect.objectContaining({ headers: { Authorization: 'Bearer runtime-only' }, credentials: 'omit', redirect: 'error' }));
  });
  it('distinguishes authorization, forbidden origin and network disconnect without exposing responses', async () => {
    for (const status of [401, 403, 500]) {
      vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('sensitive upstream response', { status })));
      await expect(new ApiClient('secret').projects()).rejects.toMatchObject({ kind: 'error', status });
    }
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('secret')));
    await expect(new ApiClient('secret').projects()).rejects.toMatchObject({ kind: 'disconnected' });
  });
  it('does not send a token to health', async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response('{"healthy":true,"version":"0.1.0"}'));
    vi.stubGlobal('fetch', fetcher);
    await new ApiClient('secret').health();
    expect(fetcher.mock.calls[0][1].headers).toEqual({});
  });
});
