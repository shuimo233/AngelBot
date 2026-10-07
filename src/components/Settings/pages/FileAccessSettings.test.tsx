import { render, screen } from '@testing-library/react';
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

    expect(await screen.findByText('AngelBot 的文件范围')).toBeInTheDocument();
    expect(screen.getByText('项目根目录')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '保存目录' })).not.toBeInTheDocument();
    expect(screen.queryByText('执行权限')).not.toBeInTheDocument();
  });

  it('makes clear that personal workspaces have no project file capability', () => {
    useWorkspacesStore.setState({
      workspaces: [{ id: 'personal', name: 'AngelBot 日常', kind: 'personal', createdAt: 1, updatedAt: 1, activeSessionId: session.id }],
      activeWorkspaceId: 'personal', loading: false, lastError: null,
    });

    render(<FileAccessSettings />);

    expect(screen.getByText('AngelBot 日常不连接项目文件')).toBeInTheDocument();
    expect(getSessionFileAccess).not.toHaveBeenCalled();
  });
});
