import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { RightPanel } from '.';
import { listWorkDir, readSessionFile, saveSessionFile } from '$lib/commands/file';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspaceActivityStore } from '$stores/workspaceActivity';
import { useWorkspacesStore } from '$stores/workspaces';

vi.mock('$lib/commands/file', () => ({
  listWorkDir: vi.fn(),
  readSessionFile: vi.fn(),
  saveSessionFile: vi.fn(),
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
      sessionId: 'session-1',
      isLoading: false,
      error: null,
      projection: null,
      refresh: vi.fn().mockResolvedValue(undefined),
      observe: vi.fn().mockReturnValue(() => {}),
    });
  });

  it('labels host-owned browser scratch without hiding ordinary log or yaml files', async () => {
    vi.mocked(listWorkDir).mockResolvedValue([
      { path: 'browser-runtime-owned', name: 'browser-runtime-owned', isDirectory: true, size: null, temporary: true },
      { path: 'notes.yml', name: 'notes.yml', isDirectory: false, size: 5 },
      { path: 'work.log', name: 'work.log', isDirectory: false, size: 4 },
    ]);
    render(<RightPanel workspaceView="files" />);
    expect(await screen.findByText('浏览器临时文件（不随备份）')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '展开 browser-runtime-owned' })).toHaveAttribute('title', expect.stringContaining('不随备份'));
    expect(screen.getByRole('button', { name: '打开 notes.yml' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '打开 work.log' })).toBeInTheDocument();
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
    await user.click(await screen.findByRole('button', { name: '打开 notes.md' }));
    expect(readSessionFile).toHaveBeenCalledWith('session-1', 'notes.md');
    expect(await screen.findByText('# Notes')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '引用文件' }));
    expect(referenced).toHaveBeenCalledTimes(1);
    expect((referenced.mock.calls[0][0] as CustomEvent).detail).toEqual({ path: 'notes.md' });
    window.removeEventListener('angelbot:reference-file', referenced);
  });

  it('reloads the same session after a restored directory is confirmed at a new location', async () => {
    const project = { id: 'project', name: 'Project', kind: 'project' as const, rootPath: 'D:/old', directoryConfirmed: false, createdAt: 1, updatedAt: 1, activeSessionId: 'session-1' };
    useWorkspacesStore.setState({ workspaces: [project], activeWorkspaceId: project.id });
    vi.mocked(listWorkDir).mockRejectedValueOnce(new Error('目录尚未确认')).mockResolvedValueOnce([{ path: 'relocated.txt', name: 'relocated.txt', isDirectory: false, size: 10 }]);
    render(<RightPanel workspaceView="files" />);
    expect(await screen.findByText('目录尚未确认')).toBeInTheDocument();
    act(() => useWorkspacesStore.setState({ workspaces: [{ ...project, rootPath: 'D:/relocated', directoryConfirmed: true }] }));
    expect(await screen.findByRole('button', { name: '打开 relocated.txt' })).toBeInTheDocument();
    expect(listWorkDir).toHaveBeenCalledTimes(2);
    expect(listWorkDir).toHaveBeenLastCalledWith('session-1');
  });

  it('opens a Main-Agent file locator within the active session boundary', async () => {
    vi.mocked(listWorkDir).mockResolvedValue([]);
    vi.mocked(readSessionFile).mockResolvedValue({ success: true, content: '# Project plan' });

    render(<RightPanel locateRequest={{ path: 'docs/plan.md', revision: 1, sessionId: 'session-1' }} />);

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
    await user.click(await screen.findByRole('button', { name: '打开 broken.md' }));

    expect(await screen.findByText('文件暂时不可读取')).toBeInTheDocument();
    expect(screen.queryByText('正在读取文件…')).not.toBeInTheDocument();
  });

  it('shows Daily files and exports original bytes even when preview is unsupported', async () => {
    useWorkspacesStore.setState({
      workspaces: [{ id: 'personal', name: 'AngelBot 日常', kind: 'personal', createdAt: 1, updatedAt: 1, activeSessionId: 'session-1' }],
      activeWorkspaceId: 'personal',
    });
    vi.mocked(listWorkDir).mockResolvedValue([{ path: 'download.bin', name: 'download.bin', isDirectory: false, size: 4 }]);
    vi.mocked(readSessionFile).mockResolvedValue({ success: false, error: '不支持文字预览' });
    vi.mocked(saveSessionFile).mockResolvedValue({ status: 'cancelled' });
    render(<RightPanel workspaceView="files" />);
    const user = userEvent.setup();
    expect(await screen.findByText('日常结果')).toBeInTheDocument();
    await user.click(await screen.findByRole('button', { name: '打开 download.bin' }));
    expect(await screen.findByText('不支持文字预览')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '另存 download.bin' }));
    expect(saveSessionFile).toHaveBeenCalledWith('session-1', 'download.bin');
    expect(screen.queryByText(/已另存/)).not.toBeInTheDocument();
    vi.mocked(saveSessionFile).mockRejectedValueOnce(new Error('目标文件已存在，未覆盖'));
    await user.click(screen.getByRole('button', { name: '另存 download.bin' }));
    expect(await screen.findByText(/目标文件已存在，未覆盖/)).toBeInTheDocument();
    vi.mocked(saveSessionFile).mockResolvedValueOnce({ status: 'saved', name: 'download-copy.bin', bytes: 4 });
    await user.click(screen.getByRole('button', { name: '另存 download.bin' }));
    expect(await screen.findByText(/已另存 download-copy.bin/)).toBeInTheDocument();
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
