import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { open } from '@tauri-apps/plugin-dialog';
import { Sidebar } from '$components/Sidebar';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';
import { useThemeStore } from '$stores/theme';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

const openDialogMock = vi.mocked(open);

const personal = { id: 'personal', name: 'AngelBot', kind: 'personal' as const, createdAt: 1, updatedAt: 2, activeSessionId: 'personal-main' };
const project = { id: 'angelbot', name: 'AngelBot', kind: 'project' as const, rootPath: 'D:\\Projects\\AngelBot', createdAt: 1, updatedAt: 3, activeSessionId: 'angelbot-main' };

describe('Sidebar workspace navigator', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    useThemeStore.setState({ layout: 'A' });
    useWorkspacesStore.setState({
      workspaces: [personal, project], activeWorkspaceId: 'personal', loading: false, lastError: null,
      loadWorkspaces: vi.fn().mockResolvedValue(undefined),
      openWorkspace: vi.fn().mockResolvedValue(true),
      createProject: vi.fn().mockResolvedValue(true),
    });
    useNavigationStore.setState({ currentPage: 'chat' });
  });

  it('separates daily navigation from project selection, never showing internal sessions', async () => {
    const user = userEvent.setup();
    render(<Sidebar />);
    expect(screen.getByRole('button', { name: '日常' })).toHaveAttribute('aria-current', 'page');
    expect(screen.queryByRole('button', { name: 'AngelBot' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '项目' }));
    expect(useWorkspacesStore.getState().openWorkspace).not.toHaveBeenCalled();
    expect(screen.getAllByRole('button', { name: 'AngelBot' })).toHaveLength(1);
    expect(screen.queryByText('personal-main')).not.toBeInTheDocument();
    expect(screen.queryByText('angelbot-main')).not.toBeInTheDocument();
  });

  it('switches layout without navigating or reopening a workspace', async () => {
    const user = userEvent.setup();
    render(<Sidebar />);
    const toggle = screen.getByRole('button', { name: '切换界面布局' });
    expect(toggle.closest('.sidebar-identity')).not.toBeNull();
    expect(toggle.closest('footer')).toBeNull();
    expect(toggle.textContent).toBe('');
    expect(toggle.querySelector('svg')).toHaveAttribute('width', '20');
    expect(toggle.querySelector('svg')).toHaveAttribute('height', '20');
    expect(toggle).toHaveAttribute('title', '收起文字侧栏，切换到 B 生活视图');
    await user.tab();
    expect(toggle).toHaveFocus();
    await user.keyboard('{Enter}');
    expect(toggle).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: '切换界面布局' })).toBe(toggle);
    expect(toggle).toHaveAttribute('title', '展开文字侧栏，切换到 A 安静工作区');
    expect(useThemeStore.getState().layout).toBe('B');
    expect(localStorage.getItem('ui-layout')).toBe('B');
    expect(screen.getByRole('button', { name: '日常' })).toHaveAttribute('aria-current', 'page');
    expect(useWorkspacesStore.getState().openWorkspace).not.toHaveBeenCalled();
    await user.click(toggle);
    expect(useThemeStore.getState().layout).toBe('A');
  });

  it('opens a workspace and returns to the single chat surface', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    useNavigationStore.setState({ currentPage: 'tasks' });
    render(<Sidebar open onClose={onClose} />);
    await user.click(screen.getByRole('button', { name: '日常' }));
    expect(useNavigationStore.getState().currentPage).toBe('chat');
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(useWorkspacesStore.getState().openWorkspace).toHaveBeenCalledWith('personal');
    await user.click(screen.getByRole('button', { name: '项目' }));
    await user.click(screen.getByRole('button', { name: 'AngelBot' }));
    expect(useWorkspacesStore.getState().openWorkspace).toHaveBeenCalledWith('angelbot');
    expect(onClose).toHaveBeenCalledTimes(3);
  });

  it('keeps the current workspace visible and explains when switching fails', async () => {
    const user = userEvent.setup();
    useNavigationStore.setState({ currentPage: 'projects' });
    useWorkspacesStore.setState({
      openWorkspace: vi.fn().mockRejectedValue(new Error('项目目录已不可用')),
    });

    render(<Sidebar />);
    await user.click(screen.getByRole('button', { name: 'AngelBot' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('项目目录已不可用');
    expect(useNavigationStore.getState().currentPage).toBe('projects');
  });

  it('prevents overlapping workspace switches while one is opening', async () => {
    const user = userEvent.setup();
    useNavigationStore.setState({ currentPage: 'projects' });
    let resolveOpen: ((accepted: boolean) => void) | undefined;
    useWorkspacesStore.setState({
      openWorkspace: vi.fn().mockImplementation(() => new Promise<boolean>((resolve) => {
        resolveOpen = resolve;
      })),
    });

    render(<Sidebar />);
    const projectButton = screen.getByRole('button', { name: 'AngelBot' });
    await user.click(projectButton);

    await waitFor(() => expect(projectButton).toBeDisabled());
    expect(projectButton).toHaveAttribute('aria-busy', 'true');
    resolveOpen?.(true);
    await waitFor(() => expect(screen.getByRole('button', { name: '日常' })).not.toBeDisabled());
    expect(useWorkspacesStore.getState().openWorkspace).toHaveBeenCalledTimes(1);
  });

  it('creates a project from a directory boundary', async () => {
    const user = userEvent.setup();
    useNavigationStore.setState({ currentPage: 'projects' });
    openDialogMock.mockResolvedValue('D:\\Projects\\new-app');
    render(<Sidebar />);
    await user.click(screen.getByTitle('新建项目'));
    await waitFor(() => {
      expect(openDialogMock).toHaveBeenCalledWith({
        directory: true,
        multiple: false,
        title: '选择项目文件夹',
      });
      expect(useWorkspacesStore.getState().createProject).toHaveBeenCalledWith('D:\\Projects\\new-app');
    });
  });

  it('opens only one folder picker for repeated project requests until it settles', async () => {
    let finish!: (selected: null) => void;
    openDialogMock.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    openDialogMock.mockResolvedValue(null);
    render(<Sidebar />);

    act(() => {
      window.dispatchEvent(new CustomEvent('new-project'));
      window.dispatchEvent(new CustomEvent('new-project'));
    });
    expect(openDialogMock).toHaveBeenCalledTimes(1);

    await act(async () => { finish(null); });
    expect(useWorkspacesStore.getState().createProject).not.toHaveBeenCalled();
    expect(openDialogMock).toHaveBeenCalledTimes(1);

    await act(async () => { window.dispatchEvent(new CustomEvent('new-project')); });
    expect(openDialogMock).toHaveBeenCalledTimes(2);
  });

  it.each(['loading', 'failed', 'empty', 'refreshing', 'stale'] as const)(
    'distinguishes an empty project list from loading or failed reads (%s)', async state => {
      useNavigationStore.setState({ currentPage: 'projects' });
      const hasProjects = state === 'refreshing' || state === 'stale';
      const failed = state === 'failed' || state === 'stale';
      const reload = vi.fn().mockResolvedValue(undefined);
      useWorkspacesStore.setState({
        workspaces: hasProjects ? [personal, project] : [],
        loading: state === 'loading' || state === 'refreshing',
        lastError: failed ? '读取工作区失败' : null,
        loadWorkspaces: reload,
      });
      render(<Sidebar />);

      const empty = screen.queryByText('添加常用文件夹，在独立项目里接着上次的事继续。');
      if (state === 'empty') expect(empty).toBeInTheDocument();
      else expect(empty).not.toBeInTheDocument();
      if (state === 'loading') expect(screen.getByRole('status')).toHaveTextContent('正在读取工作区');
      if (hasProjects) expect(screen.getByRole('button', { name: 'AngelBot' })).toBeInTheDocument();
      if (failed) {
        expect(screen.getByRole('alert')).toHaveTextContent('读取工作区失败');
        const user = userEvent.setup();
        await user.click(screen.getByRole('button', { name: '重新加载' }));
        expect(reload).toHaveBeenCalledTimes(2);
      }
    },
  );

  it.each([
    ['open', false], ['open', true], ['create', false], ['create', true],
  ] as const)('does not navigate after an unaccepted or superseded %s (accepted: %s)', async (kind, accepted) => {
    const user = userEvent.setup();
    const close = vi.fn();
    let finish!: (accepted: boolean) => void;
    const action = vi.fn(() => new Promise<boolean>((resolve) => { finish = resolve; }));
    useWorkspacesStore.setState(kind === 'open' ? { openWorkspace: action } : { createProject: action });
    useNavigationStore.getState().setPage('projects');
    openDialogMock.mockResolvedValue('D:/Projects/new-app');
    render(<Sidebar open onClose={close} />);
    const button = kind === 'open' ? screen.getByRole('button', { name: 'AngelBot' }) : screen.getByTitle('新建项目');
    await user.click(button);
    await waitFor(() => expect(action).toHaveBeenCalledTimes(1));
    if (accepted) act(() => useNavigationStore.getState().setPage('knowledge'));
    await act(async () => { finish(accepted); });
    expect(useNavigationStore.getState().currentPage).toBe(accepted ? 'knowledge' : 'projects');
    expect(close).not.toHaveBeenCalled();
  });

  it('does not create a project when the folder picker is cancelled', async () => {
    const user = userEvent.setup();
    useNavigationStore.setState({ currentPage: 'projects' });
    openDialogMock.mockResolvedValue(null);
    render(<Sidebar />);
    await user.click(screen.getByTitle('新建项目'));
    await waitFor(() => expect(openDialogMock).toHaveBeenCalledOnce());
    expect(useWorkspacesStore.getState().createProject).not.toHaveBeenCalled();
  });
});
