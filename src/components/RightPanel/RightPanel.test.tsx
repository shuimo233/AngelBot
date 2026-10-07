import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { RightPanel } from '.';
import { listWorkDir, readSessionFile } from '$lib/commands/file';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspaceActivityStore } from '$stores/workspaceActivity';
import { useWorkspacesStore } from '$stores/workspaces';

vi.mock('$lib/commands/file', () => ({
  listWorkDir: vi.fn(),
  readSessionFile: vi.fn(),
}));

describe('RightPanel file workbench', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: {
        id: 'session-1',
        title: 'Demo',
        createdAt: 1,
        updatedAt: 1,
        contextVersion: 0,
        workDir: 'D:\\Projects\\demo',
      },
    });
    useWorkspacesStore.setState({
      workspaces: [],
      activeWorkspaceId: null,
      loading: false,
      lastError: null,
    });
    useWorkspaceActivityStore.setState({
      workspaceId: 'workspace-1',
      isLoading: false,
      error: null,
      projection: null,
      refresh: vi.fn().mockResolvedValue(undefined),
    });
  });

  it('opens a session-scoped file and references it to the Main Agent', async () => {
    vi.mocked(listWorkDir).mockResolvedValue([
      { path: 'notes.md', name: 'notes.md', isDirectory: false, size: 12 },
    ]);
    vi.mocked(readSessionFile).mockResolvedValue({ success: true, content: '# Notes' });
    const referenced = vi.fn();
    window.addEventListener('angelbot:reference-file', referenced);

    render(<RightPanel workspaceView="files" />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole('button', { name: /notes\.md/i }));
    expect(readSessionFile).toHaveBeenCalledWith('session-1', 'notes.md');
    expect(await screen.findByText('# Notes')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '引用文件' }));
    expect(referenced).toHaveBeenCalledTimes(1);
    expect((referenced.mock.calls[0][0] as CustomEvent).detail).toEqual({ path: 'notes.md' });
    window.removeEventListener('angelbot:reference-file', referenced);
  });

  it('opens a Main-Agent file locator within the active session boundary', async () => {
    vi.mocked(listWorkDir).mockResolvedValue([]);
    vi.mocked(readSessionFile).mockResolvedValue({ success: true, content: '# Project plan' });

    render(<RightPanel locateRequest={{ path: 'docs/plan.md', revision: 1 }} />);

    await waitFor(() => {
      expect(readSessionFile).toHaveBeenCalledWith('session-1', 'docs/plan.md');
    });
    expect(await screen.findByText('# Project plan')).toBeInTheDocument();
  });

  it('recovers when a selected file cannot be read', async () => {
    vi.mocked(listWorkDir).mockResolvedValue([
      { path: 'broken.md', name: 'broken.md', isDirectory: false, size: 12 },
    ]);
    vi.mocked(readSessionFile).mockRejectedValue(new Error('文件暂时不可读取'));

    render(<RightPanel workspaceView="files" />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole('button', { name: /broken\.md/i }));

    expect(await screen.findByText('文件暂时不可读取')).toBeInTheDocument();
    expect(screen.queryByText('正在读取文件…')).not.toBeInTheDocument();
  });

  it('returns from an Attention summary to the existing Main-Agent conversation', async () => {
    const onClose = vi.fn();
    useWorkspacesStore.setState({
      workspaces: [{
        id: 'workspace-1',
        name: 'Demo',
        kind: 'project',
        rootPath: 'D:\\Projects\\demo',
        createdAt: 1,
        updatedAt: 1,
        activeSessionId: 'session-1',
      }],
      activeWorkspaceId: 'workspace-1',
    });
    useWorkspaceActivityStore.setState({
      projection: {
        workspaceId: 'workspace-1',
        cursor: 1,
        work: [],
        pendingDecisions: [],
        attention: {
          openCount: 1,
          message: '有工作需要继续处理，进度已安全保留。请回到主对话说明下一步。',
        },
      },
    });

    render(<RightPanel workspaceView="activity" onClose={onClose} />);

    const user = userEvent.setup();
    await user.click(await screen.findByRole('button', { name: '回到对话' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
