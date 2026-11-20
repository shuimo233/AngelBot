import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import { useSettingsStore } from './stores/settings';
import type { Workspace } from './lib/commands/workspace';
import { readSessionFile } from './lib/commands/file';
import { useSessionsStore } from './stores/sessions';
import { useThemeStore } from './stores/theme';

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
  Sidebar: () => <aside className="sidebar">{useSettingsStore((state) => state.profile.name)}<button>布局测试入口</button></aside>,
}));
vi.mock('./components/ChatArea', () => ({ ChatArea: () => <main><input aria-label="对话草稿" /></main> }));
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
vi.mock('./lib/commands/automation', () => ({
  runDueAutomations: appMocks.runDue,
  getWorkspaceAutomationOverview: vi.fn().mockResolvedValue({ items: [], truncated: false }),
}));

describe('App', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.clearAllMocks();
    localStorage.clear();
    useThemeStore.setState({ layout: 'A' });
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

  it('changes layout without remounting the active conversation or discarding its draft', () => {
    const view = render(<App />);
    const draft = screen.getByRole('textbox', { name: '对话草稿' });
    fireEvent.change(draft, { target: { value: '尚未发送的请求' } });
    const owner = appMocks.activeWorkspaceId;
    act(() => useThemeStore.getState().setLayout('B'));
    expect(view.container.querySelector('.app-shell')).toHaveAttribute('data-layout', 'B');
    expect(screen.getByRole('textbox', { name: '对话草稿' })).toBe(draft);
    expect(draft).toHaveValue('尚未发送的请求');
    act(() => useThemeStore.getState().setLayout('A'));
    expect(view.container.querySelector('.app-shell')).toHaveAttribute('data-layout', 'A');
    expect(draft).toHaveValue('尚未发送的请求');
    expect(appMocks.activeWorkspaceId).toBe(owner);
    expect(appMocks.runDue).not.toHaveBeenCalled();
  });

  it('returns focus to the compact menu when the B rail becomes a closed A sidebar', () => {
    render(<App />);
    act(() => useThemeStore.getState().setLayout('B'));
    const menu = screen.getByRole('button', { name: '打开菜单' });
    vi.spyOn(menu, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 32, 34));
    screen.getByRole('button', { name: '布局测试入口' }).focus();
    act(() => useThemeStore.getState().setLayout('A'));
    expect(menu).toHaveFocus();
  });

  it('dismisses only the compact A drawer on Escape and leaves a top-level modal in control', () => {
    const view = render(<><App /><div role="dialog" aria-modal="true"><button>弹层操作</button></div></>);
    const menu = screen.getByRole('button', { name: '打开菜单' });
    const drawerAction = screen.getByRole('button', { name: '布局测试入口' });
    const modalAction = screen.getByRole('button', { name: '弹层操作' });
    const menuRect = vi.spyOn(menu, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 36, 36));
    fireEvent.click(menu);
    modalAction.focus();
    fireEvent.keyDown(modalAction, { key: 'Escape' });
    expect(screen.getByRole('button', { name: '关闭菜单' })).toBeInTheDocument();
    expect(modalAction).toHaveFocus();

    view.rerender(<><App /></>);
    drawerAction.focus();
    act(() => useThemeStore.getState().setLayout('B'));
    fireEvent.keyDown(drawerAction, { key: 'Escape' });
    expect(drawerAction).toHaveFocus();
    expect(screen.getByRole('button', { name: '关闭菜单' })).toBeInTheDocument();

    act(() => useThemeStore.getState().setLayout('A'));
    menuRect.mockReturnValue(new DOMRect());
    fireEvent.keyDown(drawerAction, { key: 'Escape' });
    expect(screen.getByRole('button', { name: '关闭菜单' })).toBeInTheDocument();

    menuRect.mockReturnValue(new DOMRect(0, 0, 36, 36));
    fireEvent.keyDown(drawerAction, { key: 'Escape' });
    expect(screen.queryByRole('button', { name: '关闭菜单' })).not.toBeInTheDocument();
    expect(menu).toHaveFocus();
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

  it('opens workbench shortcuts from chat and honors a repeated view request after a tab change', async () => {
    appMocks.workspaces = [{ id: 'personal', name: 'AngelBot 日常', kind: 'personal', createdAt: 1, updatedAt: 1, activeSessionId: 'personal-main' }];
    appMocks.activeWorkspaceId = 'personal';
    render(<App />);

    const draft = screen.getByRole('textbox', { name: '对话草稿' });
    expect(fireEvent.keyDown(draft, { key: 'p', ctrlKey: true })).toBe(true);
    expect(screen.queryByRole('complementary', { name: '工作侧栏' })).not.toBeInTheDocument();
    expect(fireEvent.keyDown(document.body, { key: 'p', ctrlKey: true })).toBe(false);
    const files = await screen.findByRole('button', { name: '结果文件' });
    expect(files).toHaveAttribute('aria-pressed', 'true');

    fireEvent.click(screen.getByRole('button', { name: '浏览器' }));
    expect(files).toHaveAttribute('aria-pressed', 'false');
    fireEvent.keyDown(document.body, { key: 'p', ctrlKey: true });
    expect(files).toHaveAttribute('aria-pressed', 'true');
    fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(screen.queryByRole('complementary', { name: '工作侧栏' })).not.toBeInTheDocument();
    fireEvent.keyDown(document.body, { key: '.', ctrlKey: true });
    expect(screen.getByRole('button', { name: '任务' })).toHaveAttribute('aria-pressed', 'true');
  });
});
