import { afterEach, describe, expect, it, vi } from 'vitest';

const tauriMocks = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({ invoke: tauriMocks.invoke }));

import { invoke } from './invoke';

describe('invoke runtime routing', () => {
  afterEach(() => {
    tauriMocks.invoke.mockReset();
    delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  });

  it('uses native Tauri IPC inside a development WebView', async () => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      configurable: true,
      value: {},
    });
    tauriMocks.invoke.mockResolvedValue({ live: true });
    const fetchSpy = vi.spyOn(globalThis, 'fetch');

    await expect(invoke('send_message', { sessionId: 'live-session' })).resolves.toEqual({ live: true });

    expect(tauriMocks.invoke).toHaveBeenCalledWith('send_message', { sessionId: 'live-session' });
    expect(fetchSpy).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });
});
