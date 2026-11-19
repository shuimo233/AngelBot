import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { FileAccessSettings } from './FileAccessSettings';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspacesStore } from '$stores/workspaces';
import { getSessionFileAccess } from '$lib/commands/file';

vi.mock('$lib/commands/file', () => ({ getSessionFileAccess: vi.fn() }));

const session = { id: 'session-1', title: 'Demo', createdAt: Date.now(), updatedAt: Date.now(), contextVersion: 0 };

describe('FileAccessSettings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useSessionsStore.setState({ sessions: [session], activeSessionId: session.id, activeSession: session });
  });

  it('shows a project root as workspace-owned rather than editable by its session', async () => {
    useWorkspacesStore.setState({
      workspaces: [{ id: 'project-1', name: 'AngelBot', kind: 'project', rootPath: 'D:\\Projects\\AngelBot', createdAt: 1, updatedAt: 1, activeSessionId: session.id }],
      activeWorkspaceId: 'project-1', loading: false, lastError: null,
    });
    vi.mocked(getSessionFileAccess).mockResolvedValue({ workDir: 'D:\\Projects\\AngelBot', additionalReadDirs: [] });

    render(<FileAccessSettings />);

    expect(await screen.findByRole('heading', { name: 'AngelBot' })).toBeInTheDocument();
    expect(screen.getByText('项目根目录')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '保存目录' })).not.toBeInTheDocument();
    expect(screen.queryByText('执行权限')).not.toBeInTheDocument();
  });

  it('shows personal results separately without project or command capability', async () => {
    useWorkspacesStore.setState({
      workspaces: [{ id: 'personal', name: 'AngelBot 日常', kind: 'personal', createdAt: 1, updatedAt: 1, activeSessionId: session.id }],
      activeWorkspaceId: 'personal', loading: false, lastError: null,
    });

    vi.mocked(getSessionFileAccess).mockResolvedValue({ workDir: 'C:/isolated/results', additionalReadDirs: [] });
    render(<FileAccessSettings />);

    expect(screen.getByRole('heading', { name: 'AngelBot 日常' })).toBeInTheDocument();
    expect(await screen.findByText('C:/isolated/results')).toBeInTheDocument();
    expect(getSessionFileAccess).toHaveBeenCalledWith(session.id);
  });

  it('places the recovery action and unapproved status beside the restored project path', async () => {
    const user = userEvent.setup();
    const path = 'D:/Restored/Project';
    useWorkspacesStore.setState({
      workspaces: [{ id: 'restored', name: '恢复项目', kind: 'project', rootPath: path, directoryConfirmed: false, createdAt: 1, updatedAt: 1, activeSessionId: session.id }],
      activeWorkspaceId: 'restored', loading: false, lastError: null,
    });
    vi.mocked(getSessionFileAccess).mockResolvedValue({ workDir: path, additionalReadDirs: [] });
    const listener = vi.fn();
    window.addEventListener('open-settings', listener);
    try {
      render(<FileAccessSettings />);
      const row = (await screen.findByText(path)).closest<HTMLElement>('.settings-row')!;
      expect(within(row).getByText('尚未授权')).toBeInTheDocument();
      await user.click(within(row).getByRole('button', { name: '确认恢复目录' }));
      expect(listener).toHaveBeenCalledWith(expect.objectContaining({ detail: 'data' }));
    } finally {
      window.removeEventListener('open-settings', listener);
    }
  });
});
