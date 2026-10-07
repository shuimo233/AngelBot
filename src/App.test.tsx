import { act, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import { useSettingsStore } from './stores/settings';

const appMocks = vi.hoisted(() => ({
  settings: {
    apiConfigLoaded: true,
    loadApiConfig: vi.fn(),
    profile: { name: 'My Bot' },
  },
  runDue: vi.fn(),
  updates: vi.fn(),
}));

const notificationMocks = vi.hoisted(() => ({
  listen: vi.fn(),
  sendNotification: vi.fn(),
  listener: undefined as undefined | ((event: { payload: unknown }) => void | Promise<void>),
  unlisten: vi.fn(),
}));

// Keep the real App lifecycle, with deterministic children/stores and no IPC,
// model, HTTP, local storage or operating-system notification dependencies.
vi.mock('./components/Sidebar', () => ({
  Sidebar: () => <aside>{useSettingsStore((state) => state.profile.name)}</aside>,
}));
vi.mock('./components/ChatArea', () => ({ ChatArea: () => <main /> }));
vi.mock('./components/RightPanel', () => ({ RightPanel: () => null }));
vi.mock('./components/Automation/AutomationPage', () => ({ AutomationPage: () => null }));
vi.mock('./components/Library/LibraryPage', () => ({ LibraryPage: () => null }));
vi.mock('./components/Settings', () => ({ SettingsModal: () => null }));
vi.mock('./components/CommandPalette', () => ({ CommandPalette: () => null }));
vi.mock('./stores/settings', () => ({
  useSettingsStore: (selector: (state: unknown) => unknown) => selector(appMocks.settings),
}));
vi.mock('./stores/navigation', () => ({
  useNavigationStore: (selector: (state: unknown) => unknown) => selector({ currentPage: 'chat' }),
}));
vi.mock('./stores/workspaces', () => ({
  useWorkspacesStore: (selector: (state: unknown) => unknown) => selector({
    workspaces: [],
    activeWorkspaceId: null,
  }),
}));
vi.mock('./stores/theme', () => ({
  initializeTheme: () => () => {},
  useThemeStore: (selector: (state: unknown) => unknown) => selector({
    resolvedTheme: 'light',
    toggleTheme: vi.fn(),
  }),
}));
vi.mock('./lib/commands/automation', () => ({ runDueAutomations: appMocks.runDue }));
vi.mock('@tauri-apps/api/event', () => ({ listen: notificationMocks.listen }));
vi.mock('@tauri-apps/plugin-notification', () => ({
  sendNotification: notificationMocks.sendNotification,
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function getHeartbeat() {
  const timer = vi.mocked(window.setInterval);
  const index = timer.mock.calls.findIndex(([, delay]) => delay === 60_000);
  if (index < 0) throw new Error('App heartbeat was not registered');
  const callback = timer.mock.calls[index][0];
  if (typeof callback !== 'function') throw new Error('Expected a callable App heartbeat');
  return {
    id: timer.mock.results[index].value,
    tick: () => callback(undefined),
  };
}

const reminderPayload = { channel: 'reminder', title: 'Stretch', body: 'Stand up for a minute' };

describe('App', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.clearAllMocks();
    appMocks.runDue.mockReset().mockResolvedValue(0);
    notificationMocks.listen.mockReset().mockImplementation(async (_event, callback) => {
      notificationMocks.listener = callback;
      return notificationMocks.unlisten;
    });
    notificationMocks.sendNotification.mockReset();
    notificationMocks.unlisten.mockReset();
    notificationMocks.listener = undefined;
    // Preserve browser/Node overloads and waitFor's short polling timers. Tests
    // invoke the captured App heartbeat manually rather than waiting a minute.
    vi.spyOn(window, 'setInterval');
    vi.spyOn(window, 'clearInterval');
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(new Error('Unexpected network access'));
    window.addEventListener('angelbot:automations-updated', appMocks.updates);
  });

  afterEach(() => {
    window.removeEventListener('angelbot:automations-updated', appMocks.updates);
    expect(globalThis.fetch).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it('renders the app title and refreshes due work without desktop notifications', async () => {
    const view = render(<App />);
    expect(screen.getByText('My Bot')).toBeInTheDocument();
    await waitFor(() => expect(appMocks.updates).toHaveBeenCalledTimes(1));
    expect(appMocks.updates.mock.calls[0][0]).toBeInstanceOf(Event);
    expect('detail' in appMocks.updates.mock.calls[0][0]).toBe(false);
    expect(notificationMocks.listen).not.toHaveBeenCalled();
    expect(appMocks.runDue).toHaveBeenCalledTimes(1);
    expect(window.setInterval).toHaveBeenCalledWith(expect.any(Function), 60_000);
    const heartbeat = getHeartbeat();

    view.unmount();
    expect(window.clearInterval).toHaveBeenCalledWith(heartbeat.id);
    await act(async () => { heartbeat.tick(); });
    expect(appMocks.runDue).toHaveBeenCalledTimes(1);
  });

  it('shows valid desktop notifications requested by the backend and ignores invalid payloads', async () => {
    // Tauri 2 IPC is present even without the optional legacy global API.
    vi.stubGlobal('__TAURI_INTERNALS__', {});
    expect('__TAURI__' in window).toBe(false);
    const view = render(<App />);
    await waitFor(() => expect(notificationMocks.listen).toHaveBeenCalledWith(
      'notification-requested', expect.any(Function),
    ));
    await waitFor(() => expect(appMocks.runDue).toHaveBeenCalledTimes(1));
    const heartbeat = getHeartbeat();
    expect(notificationMocks.listen.mock.invocationCallOrder[0])
      .toBeLessThan(appMocks.runDue.mock.invocationCallOrder[0]);

    await act(async () => {
      for (const payload of [null, undefined, 'invalid', {}, { title: 1, body: 'text' },
        { title: 'Stretch', body: null }, { title: ' ', body: 'text' }]) {
        await notificationMocks.listener?.({ payload });
      }
      await notificationMocks.listener?.({ payload: reminderPayload });
    });
    expect(notificationMocks.sendNotification).toHaveBeenCalledTimes(1);
    expect(notificationMocks.sendNotification).toHaveBeenCalledWith({
      title: 'Stretch', body: 'Stand up for a minute',
    });

    notificationMocks.unlisten.mockRejectedValueOnce(new Error('Listener IPC already closed'));
    view.unmount();
    expect(notificationMocks.unlisten).toHaveBeenCalledTimes(1);
    expect(window.clearInterval).toHaveBeenCalledWith(heartbeat.id);
    await act(async () => { await notificationMocks.listener?.({ payload: reminderPayload }); });
    expect(notificationMocks.sendNotification).toHaveBeenCalledTimes(1);
  });

  it('cleans up a listener that finishes registration after unmount without starting the heartbeat', async () => {
    vi.stubGlobal('__TAURI_INTERNALS__', {});
    const registration = deferred<() => void>();
    notificationMocks.listen.mockImplementation((_event, callback) => {
      notificationMocks.listener = callback;
      return registration.promise;
    });
    const view = render(<App />);
    await waitFor(() => expect(notificationMocks.listen).toHaveBeenCalledTimes(1));
    expect(appMocks.runDue).not.toHaveBeenCalled();
    view.unmount();

    await act(async () => { registration.resolve(notificationMocks.unlisten); });
    expect(notificationMocks.unlisten).toHaveBeenCalledTimes(1);
    expect(appMocks.runDue).not.toHaveBeenCalled();
    expect(window.setInterval).not.toHaveBeenCalledWith(expect.any(Function), 60_000);
    expect(appMocks.updates).not.toHaveBeenCalled();
    await act(async () => { await notificationMocks.listener?.({ payload: reminderPayload }); });
    expect(notificationMocks.sendNotification).not.toHaveBeenCalled();
  });

  it('contains notification failures while ordinary due-work scans keep running', async () => {
    vi.stubGlobal('__TAURI_INTERNALS__', {});
    notificationMocks.sendNotification.mockImplementationOnce(() => {
      throw new Error('Notifications are unavailable');
    }).mockRejectedValueOnce(new Error('Notification permission denied'));
    appMocks.runDue.mockImplementationOnce(async () => {
      await notificationMocks.listener?.({ payload: reminderPayload });
      return 1;
    });
    const view = render(<App />);
    await waitFor(() => expect(appMocks.updates).toHaveBeenCalledTimes(1));
    const heartbeat = getHeartbeat();
    await act(async () => {
      await notificationMocks.listener?.({ payload: reminderPayload });
      heartbeat.tick();
    });
    await waitFor(() => expect(appMocks.updates).toHaveBeenCalledTimes(2));
    expect(notificationMocks.sendNotification).toHaveBeenCalledTimes(2);
    expect(appMocks.runDue).toHaveBeenCalledTimes(2);
    notificationMocks.unlisten.mockImplementationOnce(() => {
      throw new Error('Listener already closed');
    });
    expect(() => view.unmount()).not.toThrow();
    expect(window.clearInterval).toHaveBeenCalledWith(heartbeat.id);
  });

  it('continues scheduling when desktop notification registration fails', async () => {
    vi.stubGlobal('__TAURI_INTERNALS__', {});
    notificationMocks.listen.mockRejectedValueOnce(new Error('Notification plugin unavailable'));
    render(<App />);
    await waitFor(() => expect(appMocks.updates).toHaveBeenCalledTimes(1));
    await act(async () => { getHeartbeat().tick(); });
    expect(appMocks.runDue).toHaveBeenCalledTimes(2);
    expect(appMocks.updates).toHaveBeenCalledTimes(2);
  });

  it('does not report a failed scan as complete and retries at the next heartbeat', async () => {
    appMocks.runDue.mockRejectedValueOnce(new Error('Database temporarily unavailable'));
    render(<App />);
    await act(async () => {});
    expect(appMocks.updates).not.toHaveBeenCalled();
    await act(async () => { getHeartbeat().tick(); });
    expect(appMocks.runDue).toHaveBeenCalledTimes(2);
    expect(appMocks.updates).toHaveBeenCalledTimes(1);
  });

  it('does not emit a late scan completion after unmount', async () => {
    const scan = deferred<number>();
    appMocks.runDue.mockReturnValueOnce(scan.promise);
    const view = render(<App />);
    expect(appMocks.runDue).toHaveBeenCalledTimes(1);
    const heartbeat = getHeartbeat();
    view.unmount();
    await act(async () => {
      scan.resolve(1);
      heartbeat.tick();
    });
    expect(appMocks.updates).not.toHaveBeenCalled();
    expect(appMocks.runDue).toHaveBeenCalledTimes(1);
    expect(window.clearInterval).toHaveBeenCalledWith(heartbeat.id);
  });
});
