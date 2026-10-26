import { beforeEach, describe, expect, it, vi } from 'vitest';
import { AUTOMATIONS_UPDATED_EVENT, subscribeAutomationUpdates } from './automation-events';

const mocks = vi.hoisted(() => ({ listen: vi.fn(), isTauriRuntime: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }));
vi.mock('./invoke', () => ({ isTauriRuntime: mocks.isTauriRuntime }));

beforeEach(() => {
  vi.clearAllMocks();
  mocks.isTauriRuntime.mockReturnValue(false);
});

describe('automation refresh subscription', () => {
  it('refreshes from browser events and removes the subscription on disposal', () => {
    const refresh = vi.fn();
    const stop = subscribeAutomationUpdates(refresh);
    window.dispatchEvent(new Event(AUTOMATIONS_UPDATED_EVENT));
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(mocks.listen).not.toHaveBeenCalled();
    stop();
    window.dispatchEvent(new Event(AUTOMATIONS_UPDATED_EVENT));
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it('closes the native registration gap and refreshes from native events', async () => {
    mocks.isTauriRuntime.mockReturnValue(true);
    const unlisten = vi.fn();
    mocks.listen.mockResolvedValue(unlisten);
    const refresh = vi.fn();
    const stop = subscribeAutomationUpdates(refresh);
    await Promise.resolve();
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(mocks.listen).toHaveBeenCalledWith(AUTOMATIONS_UPDATED_EVENT, expect.any(Function));
    mocks.listen.mock.calls[0][1]();
    expect(refresh).toHaveBeenCalledTimes(2);
    stop();
    stop();
    mocks.listen.mock.calls[0][1]();
    expect(refresh).toHaveBeenCalledTimes(2);
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it('disposes a native registration that resolves after the view has closed', async () => {
    mocks.isTauriRuntime.mockReturnValue(true);
    let resolveRegistration!: (stop: () => void) => void;
    mocks.listen.mockReturnValue(new Promise<() => void>(resolve => { resolveRegistration = resolve; }));
    const refresh = vi.fn();
    const stop = subscribeAutomationUpdates(refresh);
    stop();
    const unlisten = vi.fn();
    resolveRegistration(unlisten);
    await Promise.resolve();
    expect(unlisten).toHaveBeenCalledTimes(1);
    expect(refresh).not.toHaveBeenCalled();
  });

  it('keeps browser refresh available when the native transport is unavailable', async () => {
    mocks.isTauriRuntime.mockReturnValue(true);
    mocks.listen.mockRejectedValue(new Error('native events unavailable'));
    const refresh = vi.fn();
    const stop = subscribeAutomationUpdates(refresh);
    await Promise.resolve();
    await Promise.resolve();
    window.dispatchEvent(new Event(AUTOMATIONS_UPDATED_EVENT));
    expect(refresh).toHaveBeenCalledTimes(1);
    stop();
  });
});
