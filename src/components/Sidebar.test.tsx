import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { open } from '@tauri-apps/plugin-dialog';
import { Sidebar } from '$components/Sidebar';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

const openDialogMock = vi.mocked(open);

const personal = { id: 'personal', name: 'AngelBot', kind: 'personal' as const, createdAt: 1, updatedAt: 2, activeSessionId: 'personal-main' };
const project = { id: 'angelbot', name: 'AngelBot', kind: 'project' as const, rootPath: 'D:\\Projects\\AngelBot', createdAt: 1, updatedAt: 3, activeSessionId: 'angelbot-main' };

describe('Sidebar workspace navigator', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useWorkspacesStore.setState({
      workspaces: [personal, project], activeWorkspaceId: 'personal', loading: false, lastError: null,
      loadWorkspaces: vi.fn().mockResolvedValue(undefined),
      openWorkspace: vi.fn().mockResolvedValue(true),
      createProject: vi.fn().mockResolvedValue(true),
    });
    useNavigationStore.setState({ currentPage: 'chat' });
  });

  it('shows the personal workspace and each project once, never internal sessions', () => {
    render(<Sidebar />);
    expect(screen.getByRole('button', { name: /AngelBot 日常/ })).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'AngelBot' })).toHaveLength(1);
    expect(screen.queryByText('personal-main')).not.toBeInTheDocument();
    expect(screen.queryByText('angelbot-main')).not.toBeInTheDocument();
  });

  it('opens a workspace and returns to the single chat surface', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    useNavigationStore.setState({ currentPage: 'tasks' });
    render(<Sidebar open onClose={onClose} />);
    await user.click(screen.getByRole('button', { name: '对话' }));
    expect(useNavigationStore.getState().currentPage).toBe('chat');
    expect(onClose).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: 'AngelBot' }));
    expect(useWorkspacesStore.getState().openWorkspace).toHaveBeenCalledWith('angelbot');
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it('keeps the current workspace visible and explains when switching fails', async () => {
    const user = userEvent.setup();
    useNavigationStore.setState({ currentPage: 'tasks' });
    useWorkspacesStore.setState({
      openWorkspace: vi.fn().mockRejectedValue(new Error('项目目录已不可用')),
    });

    render(<Sidebar />);
    await user.click(screen.getByRole('button', { name: 'AngelBot' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('项目目录已不可用');
    expect(useNavigationStore.getState().currentPage).toBe('tasks');
  });

  it('prevents overlapping workspace switches while one is opening', async () => {
    const user = userEvent.setup();
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
    await waitFor(() => expect(projectButton).not.toBeDisabled());
  });

  it('creates a project from a directory boundary', async () => {
    const user = userEvent.setup();
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

  it.each([
    ['open', false], ['open', true], ['create', false], ['create', true],
  ] as const)('does not navigate after an unaccepted or superseded %s (accepted: %s)', async (kind, accepted) => {
    const user = userEvent.setup();
    const close = vi.fn();
    let finish!: (accepted: boolean) => void;
    const action = vi.fn(() => new Promise<boolean>((resolve) => { finish = resolve; }));
    useWorkspacesStore.setState(kind === 'open' ? { openWorkspace: action } : { createProject: action });
    useNavigationStore.getState().setPage('memory');
    openDialogMock.mockResolvedValue('D:/Projects/new-app');
    render(<Sidebar open onClose={close} />);
    const button = kind === 'open' ? screen.getByRole('button', { name: 'AngelBot' }) : screen.getByTitle('新建项目');
    await user.click(button);
    await waitFor(() => expect(action).toHaveBeenCalledTimes(1));
    if (accepted) act(() => useNavigationStore.getState().setPage('knowledge'));
    await act(async () => { finish(accepted); });
    expect(useNavigationStore.getState().currentPage).toBe(accepted ? 'knowledge' : 'memory');
    expect(close).not.toHaveBeenCalled();
  });

  it('does not create a project when the folder picker is cancelled', async () => {
    const user = userEvent.setup();
    openDialogMock.mockResolvedValue(null);
    render(<Sidebar />);
    await user.click(screen.getByTitle('新建项目'));
    await waitFor(() => expect(openDialogMock).toHaveBeenCalledOnce());
    expect(useWorkspacesStore.getState().createProject).not.toHaveBeenCalled();
  });
});
