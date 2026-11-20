import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { RightPanel } from '.';
import { listWorkDir, openDailyResultsFolder, readSessionFile, saveSessionFile } from '$lib/commands/file';
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
  openDailyResultsFolder: vi.fn(),
  readSessionFile: vi.fn(),
  saveSessionFile: vi.fn(),
}));

describe('RightPanel file workbench', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.removeItem('workbenchWidth');
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

  it('opens to tasks and keeps the selected view clear when tabs or shortcuts change it', async () => {
    const user = userEvent.setup();
    vi.mocked(listWorkDir).mockResolvedValue([]);
    const { rerender } = render(<RightPanel />);
    const heading = screen.getByRole('heading', { name: '工作台' });
    const tasks = screen.getByRole('button', { name: '任务', pressed: true });
    expect(tasks).toHaveAttribute('aria-keyshortcuts', 'Control+J');
    expect(screen.getByRole('heading', { name: '任务与委派' })).toBeVisible();

    rerender(<RightPanel workspaceView="files" workspaceViewRevision={1} />);
    expect(await screen.findByText('目录为空')).toBeVisible();
    expect(screen.getByRole('button', { name: '文件', pressed: true })).toBeVisible();

    await user.click(screen.getByRole('button', { name: '浏览器' }));
    expect(screen.getByRole('button', { name: '浏览器', pressed: true })).toBeVisible();
    expect(screen.getByRole('button', { name: '打开网址' })).toHaveTextContent('打开');
    expect(heading).toBeVisible();

    rerender(<RightPanel workspaceView="files" workspaceViewRevision={2} />);
    expect(await screen.findByRole('button', { name: '文件', pressed: true })).toBeVisible();
  });

  it('uses compact list and preview views without remounting the folder tree', async () => {
    const media = { matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() };
    const matchMedia = vi.fn((query: string) => ({ ...media, matches: query.includes('1050px') }));
    vi.stubGlobal('matchMedia', matchMedia);
    try {
      const user = userEvent.setup();
      const name = '需要完整显示且允许换行的长期项目交付说明.md';
      const path = `docs/${name}`;
      vi.mocked(listWorkDir).mockImplementation(async (_session, folder) => folder === 'docs'
        ? [{ path, name, isDirectory: false, size: 12 }]
        : [{ path: 'docs', name: 'docs', isDirectory: true, size: null }]);
      vi.mocked(readSessionFile).mockResolvedValue({ success: true, content: '# 报告正文' });
      render(<RightPanel workspaceView="files" onClose={vi.fn()} />);

      expect(screen.queryByRole('separator')).not.toBeInTheDocument();
      await user.click(await screen.findByRole('button', { name: '展开 docs' }));
      const file = await screen.findByRole('button', { name: `打开 ${name}` });
      await user.click(file);
      expect(await screen.findByRole('heading', { name: path })).toHaveFocus();
      expect(screen.getByRole('heading', { name: '报告正文' })).toBeVisible();
      expect(screen.queryByRole('button', { name: `打开 ${name}` })).not.toBeInTheDocument();

      await user.click(screen.getByRole('button', { name: /返回文件列表/ }));
      expect(screen.getByRole('button', { name: `打开 ${name}` })).toBe(file);
      expect(file).toHaveFocus();
      expect(screen.getByRole('button', { name: '收起 docs' })).toBeVisible();
      expect(listWorkDir).toHaveBeenCalledTimes(2);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('keeps the keyboard-adjusted workbench width when the panel is reopened', async () => {
    const user = userEvent.setup();
    const view = render(<RightPanel />);
    const separator = screen.getByRole('separator', { name: '调整工作台宽度' });
    separator.focus();
    await user.keyboard('{ArrowLeft}{ArrowLeft}');
    expect(separator).toHaveAttribute('aria-valuenow', '368');
    view.unmount();
    render(<RightPanel />);
    expect(screen.getByRole('separator', { name: '调整工作台宽度' })).toHaveAttribute('aria-valuenow', '368');
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
    const markdown = '# Notes\n\n| 项目 | 金额 |\n| --- | ---: |\n| 合计 | 436.70 |\n\n![报告图](https://example.invalid/remote.png)';
    vi.mocked(readSessionFile).mockResolvedValue({ success: true, content: markdown });
    const referenced = vi.fn();
    window.addEventListener('angelbot:reference-file', referenced);

    render(<RightPanel workspaceView="files" />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole('button', { name: '打开 notes.md' }));
    expect(readSessionFile).toHaveBeenCalledWith('session-1', 'notes.md');
    expect(await screen.findByRole('heading', { name: 'Notes' })).toBeInTheDocument();
    expect(screen.getByRole('table')).toHaveTextContent('436.70');
    expect(screen.queryByRole('img', { name: '报告图' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '查看源码' }));
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
    expect(screen.getByText(/^# Notes/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '阅读预览' }));
    expect(screen.getByRole('table')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '引用文件' }));
    expect(referenced).toHaveBeenCalledTimes(1);
    expect((referenced.mock.calls[0][0] as CustomEvent).detail).toEqual({ path: 'notes.md' });
    window.removeEventListener('angelbot:reference-file', referenced);
  });

  it('highlights code with line numbers and copies the exact preview source', async () => {
    const user = userEvent.setup();
    const content = 'const name = "保持源码";\r\n\r\nconsole.log(name);\r\n';
    const writeText = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
    vi.mocked(listWorkDir).mockResolvedValue([{ path: 'sample.ts', name: 'sample.ts', isDirectory: false, size: 42 }]);
    vi.mocked(readSessionFile).mockResolvedValue({ success: true, content });
    const { container } = render(<RightPanel workspaceView="files" />);
    await user.click(await screen.findByRole('button', { name: '打开 sample.ts' }));
    const copy = await screen.findByRole('button', { name: '复制原文' });
    const source = container.querySelector('.file-preview-source');
    expect(source?.querySelectorAll('.react-syntax-highlighter-line-number')).toHaveLength(3);
    expect(Array.from(source?.querySelectorAll<HTMLElement>('.token') ?? []).some(
      token => token.textContent === 'const' && token.style.color,
    )).toBe(true);
    await user.click(copy);
    expect(writeText).toHaveBeenCalledWith(content);
    expect(screen.getByRole('status')).toHaveTextContent('已复制原文');
  });

  it('keeps files with many lines in an exact plain text preview', async () => {
    const content = 'const value = 1;\n'.repeat(2_001);
    vi.mocked(listWorkDir).mockResolvedValue([]);
    vi.mocked(readSessionFile).mockResolvedValue({ success: true, content });
    const { container } = render(<RightPanel locateRequest={{ path: 'large.js', revision: 1, sessionId: 'session-1' }} />);
    await screen.findByRole('button', { name: '复制原文' });
    expect(container.querySelector('.file-preview-content')?.textContent).toBe(content);
    expect(container.querySelector('.file-preview-source')).not.toBeInTheDocument();
  });

  it('clears the preview when refreshing its directory is rejected', async () => {
    const user = userEvent.setup();
    vi.mocked(listWorkDir).mockResolvedValue([
      { path: 'notes.txt', name: 'notes.txt', isDirectory: false, size: 12 },
    ]);
    vi.mocked(readSessionFile).mockResolvedValue({ success: true, content: '旧目录预览' });
    render(<RightPanel workspaceView="files" />);
    await user.click(await screen.findByRole('button', { name: '打开 notes.txt' }));
    expect(await screen.findByText('旧目录预览')).toBeVisible();

    vi.mocked(listWorkDir).mockRejectedValueOnce(new Error('项目目录已指向其他位置。'));
    await user.click(screen.getByRole('button', { name: '刷新目录' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('项目目录已指向其他位置');
    expect(screen.queryByText('旧目录预览')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '引用文件' })).not.toBeInTheDocument();
  });

  it.each(['tree', 'locator'] as const)('marks bounded previews from %s without truncating original exports', async (entrypoint) => {
    const original = 'a'.repeat(500_000) + 'ORIGINAL_TAIL';
    vi.mocked(listWorkDir).mockResolvedValue(['long.txt', 'edge.txt', 'short.txt'].map((path) => ({
      path, name: path, isDirectory: false, size: original.length,
    })));
    vi.mocked(readSessionFile).mockImplementation(async (_session, path) => ({ success: true,
      content: path === 'long.txt' ? original : path === 'edge.txt' ? 'b'.repeat(500_000) : 'short preview',
    }));
    vi.mocked(saveSessionFile).mockResolvedValue({ status: 'saved', name: 'complete.txt', bytes: original.length });
    const { container } = render(<RightPanel workspaceView="files" locateRequest={entrypoint === 'locator'
      ? { path: 'long.txt', revision: 1, sessionId: 'session-1' } : undefined} />);
    const user = userEvent.setup();
    if (entrypoint === 'tree') await user.click(await screen.findByRole('button', { name: '打开 long.txt' }));
    expect(await screen.findByText(/仅显示开头部分/)).toBeVisible();
    expect(container.querySelector('.file-preview-content')?.textContent).toHaveLength(500_000);
    expect(container.querySelector('.file-preview-content')).not.toHaveTextContent('ORIGINAL_TAIL');
    await user.click(screen.getByRole('button', { name: '另存为…' }));
    expect(saveSessionFile).toHaveBeenCalledWith('session-1', 'long.txt');
    expect(await screen.findByText(/已另存 complete.txt/)).toHaveTextContent(`${original.length} 字节`);
    expect(original.endsWith('ORIGINAL_TAIL')).toBe(true);
    await user.click(screen.getByRole('button', { name: '打开 edge.txt' }));
    await waitFor(() => expect(container.querySelector('.file-preview-content')?.textContent?.[0]).toBe('b'));
    expect(screen.queryByText(/仅显示开头部分/)).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '打开 short.txt' }));
    expect(await screen.findByText('short preview')).toBeVisible();
    expect(screen.queryByText(/仅显示开头部分/)).not.toBeInTheDocument();
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

  it('opens a file reference and reveals only its ancestor folders in the active session', async () => {
    const name = '需要完整显示且允许换行的长期项目交付说明.md';
    const path = `docs/项目资料/${name}`;
    const folder = (path: string, name: string) => ({ path, name, isDirectory: true, size: null });
    vi.mocked(listWorkDir).mockImplementation(async (_session, directory) => {
      if (directory === 'docs') return [folder('docs/项目资料', '项目资料'), folder('docs/archive', 'archive')];
      if (directory === 'docs/项目资料') return [{ path, name, isDirectory: false, size: 12 }];
      return [folder('docs', 'docs'), folder('src', 'src')];
    });
    vi.mocked(readSessionFile).mockResolvedValue({ success: true, content: '# Project plan' });

    const { rerender } = render(<RightPanel locateRequest={{ path, revision: 1, sessionId: 'session-1' }} />);

    const selected = await screen.findByRole('button', { name: `打开 ${name}`, current: true });
    expect(selected).toBeVisible();
    expect(screen.getByRole('button', { name: '收起 docs' })).toBeVisible();
    expect(screen.getByRole('button', { name: '收起 项目资料' })).toBeVisible();
    expect(screen.getByRole('button', { name: '展开 archive' })).toBeVisible();
    expect(screen.getByRole('button', { name: '展开 src' })).toBeVisible();
    expect(await screen.findByRole('heading', { name: 'Project plan' })).toBeVisible();
    expect(screen.getByRole('heading', { name: path })).toHaveAttribute('title', path);
    expect(vi.mocked(listWorkDir).mock.calls).toEqual([['session-1'], ['session-1', 'docs'], ['session-1', 'docs/项目资料']]);
    expect(vi.mocked(readSessionFile).mock.calls).toEqual([['session-1', path]]);

    rerender(<RightPanel locateRequest={{ path: 'src/private.md', revision: 2, sessionId: 'another-session' }} />);
    expect(selected).toHaveAttribute('aria-current', 'true');
    expect(screen.getByRole('heading', { name: 'Project plan' })).toBeVisible();
    expect(listWorkDir).toHaveBeenCalledTimes(3);
    expect(readSessionFile).toHaveBeenCalledTimes(1);

    vi.mocked(readSessionFile).mockResolvedValueOnce({ success: false, error: '不支持文字预览' });
    rerender(<RightPanel locateRequest={{ path, revision: 3, sessionId: 'session-1' }} />);
    expect(await screen.findByText('不支持文字预览')).toBeVisible();
    expect(selected).toHaveAttribute('aria-current', 'true');
    expect(screen.queryByRole('heading', { name: 'Project plan' })).not.toBeInTheDocument();
    expect(listWorkDir).toHaveBeenCalledTimes(3);
    expect(readSessionFile).toHaveBeenCalledTimes(2);
  });

  it('filters previously read file names and paths without traversing unopened directories', async () => {
    const user = userEvent.setup();
    vi.mocked(listWorkDir).mockImplementation(async (_session, directory) => directory === 'docs'
      ? [{ path: 'docs/会议摘要.md', name: '会议摘要.md', isDirectory: false, size: 12 }]
      : [{ path: 'docs', name: 'docs', isDirectory: true, size: null },
        { path: 'notes.md', name: 'notes.md', isDirectory: false, size: 12 }]);
    vi.mocked(readSessionFile).mockResolvedValue({ success: true, content: '# 会议记录' });
    render(<RightPanel workspaceView="files" />);

    const docs = await screen.findByRole('button', { name: '展开 docs' });
    const filter = screen.getByRole('searchbox', { name: '筛选已读取的文件' });
    await user.type(filter, 'DOCS/会议');
    expect(screen.queryByRole('button', { name: /会议摘要\.md/ })).not.toBeInTheDocument();
    expect(vi.mocked(listWorkDir).mock.calls).toEqual([['session-1']]);

    await user.clear(filter);
    await user.click(docs);
    expect(await screen.findByRole('button', { name: '打开 会议摘要.md' })).toBeVisible();
    await user.type(filter, 'DOCS/会议');
    const match = screen.getByRole('button', { name: /会议摘要\.md.*docs/ });
    expect(match).toBeVisible();
    expect(screen.queryByRole('button', { name: /notes\.md/ })).not.toBeInTheDocument();
    expect(vi.mocked(listWorkDir).mock.calls).toEqual([['session-1'], ['session-1', 'docs']]);
    await user.click(match);
    expect(await screen.findByRole('heading', { name: '会议记录' })).toBeVisible();
    expect(readSessionFile).toHaveBeenCalledWith('session-1', 'docs/会议摘要.md');
    await user.clear(filter);
    expect(screen.getByRole('button', { name: '收起 docs' })).toBeVisible();
    expect(screen.getByRole('button', { name: '打开 会议摘要.md', current: true })).toBeVisible();
  });

  it('batches long directories while keeping undisplayed files searchable and locatable', async () => {
    const user = userEvent.setup();
    const files = Array.from({ length: 90 }, (_, index) => {
      const path = `file-${String(index + 1).padStart(3, '0')}.txt`;
      return { path, name: path, isDirectory: false, size: 12 };
    });
    vi.mocked(listWorkDir).mockResolvedValue(files);
    vi.mocked(readSessionFile).mockResolvedValue({ success: true, content: '第 90 个文件' });
    const { rerender } = render(<RightPanel workspaceView="files" />);

    await screen.findByRole('button', { name: '打开 file-001.txt' });
    expect(screen.getAllByRole('button', { name: /^打开 file-\d+\.txt$/ })).toHaveLength(80);
    expect(screen.queryByRole('button', { name: '打开 file-090.txt' })).not.toBeInTheDocument();
    const filter = screen.getByRole('searchbox', { name: '筛选已读取的文件' });
    await user.type(filter, 'file-090');
    expect(screen.getByRole('button', { name: 'file-090.txt' })).toBeVisible();
    expect(listWorkDir).toHaveBeenCalledTimes(1);
    await user.clear(filter);

    rerender(<RightPanel workspaceView="files" locateRequest={{ path: 'file-090.txt', revision: 1, sessionId: 'session-1' }} />);
    expect(await screen.findByRole('button', { name: '打开 file-090.txt', current: true })).toBeVisible();
    expect(screen.queryByRole('button', { name: '打开 file-089.txt' })).not.toBeInTheDocument();
    expect(await screen.findByText('第 90 个文件')).toBeVisible();
    await user.click(screen.getByRole('button', { name: /显示更多/ }));
    expect(screen.getAllByRole('button', { name: /^打开 file-\d+\.txt$/ })).toHaveLength(90);
    expect(screen.queryByRole('button', { name: /显示更多/ })).not.toBeInTheDocument();
    expect(vi.mocked(listWorkDir).mock.calls).toEqual([['session-1']]);
  });

  it('keeps a located folder beyond the first page open when selecting its sibling file', async () => {
    const user = userEvent.setup();
    const path = 'z-reports/report.md';
    const sibling = 'z-reports/sibling.md';
    const entry = (path: string) => ({ path, name: path.split('/').at(-1)!, isDirectory: false, size: 12 });
    const root = Array.from({ length: 80 }, (_, index) => ({ path: `a-${index}`, name: `a-${index}`, isDirectory: true, size: null }));
    vi.mocked(listWorkDir).mockImplementation(async (_session, directory) => directory === 'z-reports'
      ? [entry(path), entry(sibling)] : [...root, { path: 'z-reports', name: 'z-reports', isDirectory: true, size: null }]);
    vi.mocked(readSessionFile).mockImplementation(async (_session, file) => ({ success: true, content: `# ${file}` }));
    render(<RightPanel locateRequest={{ path, revision: 1, sessionId: 'session-1' }} />);

    expect(await screen.findByRole('button', { name: '打开 report.md', current: true })).toBeVisible();
    await user.click(screen.getByRole('button', { name: '打开 sibling.md' }));
    expect(await screen.findByRole('button', { name: '打开 sibling.md', current: true })).toBeVisible();
    expect(screen.getByRole('button', { name: '收起 z-reports' })).toBeVisible();
    expect(readSessionFile).toHaveBeenLastCalledWith('session-1', sibling);
    expect(vi.mocked(listWorkDir).mock.calls).toEqual([['session-1'], ['session-1', 'z-reports']]);
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

  it('opens the current browser page with opener isolation without interpreting a null handle as failure', async () => {
    const open = vi.spyOn(window, 'open').mockReturnValue(null);
    try {
      const user = userEvent.setup();
      render(<RightPanel workspaceView="browser" />);
      await user.type(screen.getByRole('textbox', { name: '网址' }), 'https://example.test/fixture');
      await user.click(screen.getByRole('button', { name: '打开网址' }));
      await user.click(screen.getByRole('button', { name: '弹出窗口' }));
      expect(open).toHaveBeenCalledTimes(1);
      expect(open).toHaveBeenCalledWith('https://example.test/fixture', '_blank', 'noopener,noreferrer');
      expect(screen.queryByText(/浏览器阻止了弹出窗口/)).not.toBeInTheDocument();
    } finally {
      open.mockRestore();
    }
  });

  it('recovers when a selected file cannot be read', async () => {
    vi.mocked(listWorkDir).mockResolvedValue([
      { path: 'broken.md', name: 'broken.md', isDirectory: false, size: 12 },
    ]);
    vi.mocked(readSessionFile).mockRejectedValue(new Error('文件暂时不可读取'));

    render(<RightPanel workspaceView="files" />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole('button', { name: '打开 broken.md' }));

    const feedback = await screen.findByRole('alert');
    expect(feedback).toHaveTextContent('文件暂时不可读取');
    expect(feedback).toHaveClass('ui-notice--inline', 'ui-notice--danger');
    expect(feedback.previousElementSibling).toHaveClass('workbench-file-toolbar');
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
    await user.click(screen.getByRole('button', { name: '打开日常结果目录' }));
    expect(openDailyResultsFolder).toHaveBeenCalledOnce();
    expect(openProjectFolder).not.toHaveBeenCalled();
    expect(useWorkspacesStore.getState().activeWorkspaceId).toBe('personal');
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
