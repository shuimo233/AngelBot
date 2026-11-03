import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { RightPanel } from '.';
import { listWorkDir, readSessionFile, saveSessionFile } from '$lib/commands/file';
import { getWorkspaceAutomationOverview } from '$lib/commands/automation';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspaceActivityStore } from '$stores/workspaceActivity';
import { useWorkspacesStore } from '$stores/workspaces';
import { useNavigationStore } from '$stores/navigation';
import { openProjectFolder } from '$lib/project-folder';

vi.mock('$lib/project-folder', () => ({ openProjectFolder: vi.fn() }));

vi.mock('$lib/commands/automation', () => ({ getWorkspaceAutomationOverview: vi.fn() }));

vi.mock('$lib/commands/file', () => ({
  listWorkDir: vi.fn(),
  readSessionFile: vi.fn(),
  saveSessionFile: vi.fn(),
}));

describe('RightPanel file workbench', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(openProjectFolder).mockResolvedValue(false);
    vi.mocked(getWorkspaceAutomationOverview).mockResolvedValue({ items: [], truncated: false });
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

  it('offers the shared folder-opening flow without replacing current files on cancellation or failure', async () => {
    const user = userEvent.setup();
    vi.mocked(listWorkDir).mockResolvedValue([{ path: 'notes.md', name: 'notes.md', isDirectory: false, size: 4 }]);
    render(<RightPanel workspaceView="files" />);
    expect(await screen.findByRole('heading', { name: '资源管理器' })).toBeInTheDocument();
    await screen.findByRole('button', { name: '打开 notes.md' });
    await user.click(screen.getByRole('button', { name: '打开文件夹' }));
    expect(openProjectFolder).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: '打开 notes.md' })).toBeVisible();
    vi.mocked(openProjectFolder).mockRejectedValueOnce(new Error('无法打开所选文件夹'));
    await user.click(screen.getByRole('button', { name: '打开文件夹' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('无法打开所选文件夹');
    expect(screen.getByRole('button', { name: '打开 notes.md' })).toBeVisible();
  });

  it('lazily expands the folder tree and does not carry an old preview into a different root', async () => {
    const user = userEvent.setup();
    vi.mocked(listWorkDir).mockImplementation(async (_session, path) => path === 'docs'
      ? [{ path: 'docs/notes.md', name: 'notes.md', isDirectory: false, size: 4 }]
      : [{ path: 'docs', name: 'docs', isDirectory: true, size: null }]);
    vi.mocked(readSessionFile).mockResolvedValue({ success: true, content: '原项目内容' });
    render(<RightPanel workspaceView="files" />);
    await screen.findByRole('button', { name: '展开 docs' });
    expect(listWorkDir).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: '展开 docs' }));
    await user.click(await screen.findByRole('button', { name: '打开 notes.md' }));
    expect(await screen.findByText('原项目内容')).toBeVisible();
    expect(readSessionFile).toHaveBeenCalledWith('session-1', 'docs/notes.md');
    act(() => useSessionsStore.setState({ activeSessionId: 'new-main', activeSession: { id: 'new-main', title: 'New', createdAt: 1, updatedAt: 1, contextVersion: 0 } }));
    expect(screen.queryByText('原项目内容')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '打开 notes.md' })).not.toBeInTheDocument();
    await screen.findByRole('button', { name: '展开 docs' });
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
    const path = 'docs/项目资料/需要完整显示且允许换行的长期项目交付说明.md';
    vi.mocked(listWorkDir).mockResolvedValue([]);
    vi.mocked(readSessionFile).mockResolvedValue({ success: true, content: '# Project plan' });

    render(<RightPanel locateRequest={{ path, revision: 1, sessionId: 'session-1' }} />);

    await waitFor(() => {
      expect(readSessionFile).toHaveBeenCalledWith('session-1', path);
    });
    expect(await screen.findByText('# Project plan')).toBeInTheDocument();
    expect(screen.getByText(path)).toHaveAttribute('title', path);
  });

  it('keeps a visible address label and existing browser controls in the compact toolbar', async () => {
    render(<RightPanel workspaceView="browser" />);
    const user = userEvent.setup();
    expect(screen.getByRole('heading', { name: '浏览器' })).toBeInTheDocument();
    const address = screen.getByRole('textbox', { name: '网址' });
    expect(screen.getByText('网址', { selector: 'label' })).toHaveAttribute('for', address.id);
    expect(screen.getByRole('button', { name: '后退' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '前进' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '刷新' })).toBeDisabled();
    await user.type(address, 'file:///private.txt');
    await user.click(screen.getByRole('button', { name: '打开网址' }));
    expect(screen.getByText('仅支持 http:// 或 https:// 网站。')).toBeInTheDocument();
    expect(screen.queryByTitle('AngelBot 浏览器')).not.toBeInTheDocument();
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

  it('keeps automation history and the return to Main action available in the task workbench', async () => {
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
    vi.mocked(getWorkspaceAutomationOverview).mockResolvedValue({ items: [{
      id: 'ended-automation', title: '旧自动化记录', triggerKind: 'once', triggerValue: '2026-10-01T16:00:00',
      enabled: false, executorKind: 'agent', workspaceId: 'workspace-1', nextRunAt: null,
      lastRunAt: null, lastSkippedAt: null, maaConfig: null,
      latestRun: { id: 'ended-run', status: 'completed', summary: '历史结果仍可回看', exitCode: null, startedAt: 1 },
    }], truncated: false });

    render(<RightPanel workspaceView="activity" onClose={onClose} />);

    const user = userEvent.setup();
    await user.click(await screen.findByText('事项回看'));
    expect(screen.getByText('历史结果仍可回看')).toBeVisible();
    expect(getWorkspaceAutomationOverview).toHaveBeenCalledWith('workspace-1');
    await user.click(screen.getByRole('button', { name: '查看 旧自动化记录 的运行记录' }));
    expect(useNavigationStore.getState().automationTarget).toMatchObject({ automationId: 'ended-automation', workspaceId: 'workspace-1' });
    await user.click(await screen.findByRole('button', { name: '回到对话' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
