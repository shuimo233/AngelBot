import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import { useSettingsStore } from './stores/settings';
import type { Workspace } from './lib/commands/workspace';
import { readSessionFile } from './lib/commands/file';
import { useSessionsStore } from './stores/sessions';

const appMocks = vi.hoisted(() => ({
  settings: {
    apiConfigLoaded: true,
    loadApiConfig: vi.fn(),
    profile: { name: 'My Bot' },
  },
  runDue: vi.fn(),
  workspaces: new Array<Workspace>(),
  activeWorkspaceId: '',
}));

// Keep the real App lifecycle, with deterministic children/stores and no IPC,
// model, HTTP, local storage or operating-system notification dependencies.
vi.mock('./components/Sidebar', () => ({
  Sidebar: () => <aside>{useSettingsStore((state) => state.profile.name)}</aside>,
}));
vi.mock('./components/ChatArea', () => ({ ChatArea: () => <main /> }));
vi.mock('./components/WorkspaceActivityCapsule', () => ({ WorkspaceActivityCapsule: () => null }));
vi.mock('./lib/commands/file', () => ({
  listWorkDir: vi.fn(async () => []),
  readSessionFile: vi.fn(async () => ({ success: true, content: 'File preview' })),
  saveSessionFile: vi.fn(),
}));
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
    workspaces: appMocks.workspaces,
    activeWorkspaceId: appMocks.activeWorkspaceId,
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

describe('App', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.clearAllMocks();
    appMocks.runDue.mockReset().mockResolvedValue(0);
    appMocks.workspaces = [];
    appMocks.activeWorkspaceId = '';
    useSessionsStore.setState({ sessions: [], activeSessionId: null, activeSession: null });
    vi.spyOn(window, 'setInterval');
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(new Error('Unexpected network access'));
  });

  afterEach(() => {
    expect(globalThis.fetch).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it('does not make due scheduling depend on mounting or remounting the page', async () => {
    const view = render(<App />);
    await act(async () => {});
    view.unmount();
    render(<App />);
    await act(async () => {});
    expect(appMocks.runDue).not.toHaveBeenCalled();
    expect(window.setInterval).not.toHaveBeenCalledWith(expect.any(Function), 60_000);
  });

  it('renders the app title and keeps native file drops out of navigation', () => {
    const view = render(<App />);
    expect(screen.getByText('My Bot')).toBeInTheDocument();
    const outsideComposer = screen.getByRole('main');
    expect(fireEvent.dragOver(outsideComposer, { dataTransfer: { types: ['Files'], files: [] } })).toBe(false);
    expect(fireEvent.drop(outsideComposer, { dataTransfer: { types: ['Files'], files: [new File(['private'], 'ignored.txt')] } })).toBe(false);
    expect(fireEvent.drop(outsideComposer, { dataTransfer: { types: ['text/plain'], files: [] } })).toBe(true);
    view.unmount();
    expect(appMocks.runDue).not.toHaveBeenCalled();
  });

  it('never dispatches an old file locator into a new workspace or restored directory scope', async () => {
    appMocks.workspaces = [{ id: 'personal', name: 'AngelBot 日常', kind: 'personal', createdAt: 1, updatedAt: 1, activeSessionId: 'personal-main' }];
    appMocks.activeWorkspaceId = 'personal';
    useSessionsStore.setState({ activeSessionId: 'personal-main', activeSession: { id: 'personal-main', title: '', createdAt: 1, updatedAt: 1, contextVersion: 0 } });
    const view = render(<App />);
    act(() => { window.dispatchEvent(new CustomEvent('angelbot:locate-file', { detail: { path: 'receipt.json' } })); });
    expect(await screen.findByText('receipt.json')).toBeInTheDocument();
    expect(readSessionFile).toHaveBeenCalledWith('personal-main', 'receipt.json');
    const project: Workspace = { id: 'project', name: 'Project', kind: 'project', rootPath: 'D:/owned', createdAt: 1, updatedAt: 1, activeSessionId: 'project-main' };
    act(() => {
      appMocks.workspaces.push(project);
      appMocks.activeWorkspaceId = 'project';
      useSessionsStore.setState({ activeSessionId: 'project-main', activeSession: { id: 'project-main', title: '', createdAt: 1, updatedAt: 1, contextVersion: 0 } });
      view.rerender(<App />);
    });
    expect(readSessionFile).not.toHaveBeenCalledWith('project-main', 'receipt.json');
    expect(screen.getByRole('complementary', { name: '工作侧栏' })).not.toHaveTextContent('receipt.json');
    act(() => { window.dispatchEvent(new CustomEvent('angelbot:locate-file', { detail: { path: 'plan.md' } })); });
    expect(await screen.findByText('plan.md')).toBeInTheDocument();
    expect(readSessionFile).toHaveBeenCalledWith('project-main', 'plan.md');
    project.rootPath = 'D:/relocated'; project.directoryConfirmed = true;
    view.rerender(<App />);
    expect(screen.getByRole('complementary', { name: '工作侧栏' })).not.toHaveTextContent('plan.md');
    await waitFor(() => expect(readSessionFile).toHaveBeenCalledTimes(2));
  });
});
