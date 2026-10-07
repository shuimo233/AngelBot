import { render, screen, waitFor } from '@testing-library/react';
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
      openWorkspace: vi.fn().mockResolvedValue(undefined),
      createProject: vi.fn().mockResolvedValue(undefined),
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
    useNavigationStore.setState({ currentPage: 'tasks' });
    render(<Sidebar />);
    await user.click(screen.getByRole('button', { name: 'AngelBot' }));
    expect(useWorkspacesStore.getState().openWorkspace).toHaveBeenCalledWith('angelbot');
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
    let resolveOpen: (() => void) | undefined;
    useWorkspacesStore.setState({
      openWorkspace: vi.fn().mockImplementation(() => new Promise<void>((resolve) => {
        resolveOpen = resolve;
      })),
    });

    render(<Sidebar />);
    const projectButton = screen.getByRole('button', { name: 'AngelBot' });
    await user.click(projectButton);

    await waitFor(() => expect(projectButton).toBeDisabled());
    expect(projectButton).toHaveAttribute('aria-busy', 'true');
    resolveOpen?.();
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

  it('does not create a project when the folder picker is cancelled', async () => {
    const user = userEvent.setup();
    openDialogMock.mockResolvedValue(null);
    render(<Sidebar />);
    await user.click(screen.getByTitle('新建项目'));
    await waitFor(() => expect(openDialogMock).toHaveBeenCalledOnce());
    expect(useWorkspacesStore.getState().createProject).not.toHaveBeenCalled();
  });
});
