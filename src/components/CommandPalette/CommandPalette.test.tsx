import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { CommandPalette } from './index';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';

const personal = { id: 'personal', name: 'AngelBot', kind: 'personal' as const, createdAt: 1, updatedAt: 2, activeSessionId: 'personal-main' };
const project = { id: 'project', name: 'Design Notes', kind: 'project' as const, rootPath: 'D:\\Projects\\Design Notes', createdAt: 1, updatedAt: 3, activeSessionId: 'project-main' };
const realOpenWorkspace = useWorkspacesStore.getState().openWorkspace;

describe('CommandPalette', () => {
  const openWorkspace = vi.fn().mockResolvedValue(true);

  beforeEach(() => {
    vi.clearAllMocks();
    useNavigationStore.setState({ currentPage: 'chat' });
    useWorkspacesStore.setState({
      workspaces: [personal, project],
      activeWorkspaceId: 'personal',
      loading: false,
      lastError: null,
      loadWorkspaces: vi.fn().mockResolvedValue(undefined),
      openWorkspace,
      createProject: vi.fn().mockResolvedValue(true),
    });
  });

  it('switches projects through the single workspace-level conversation', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<CommandPalette isOpen onClose={onClose} />);

    await user.click(screen.getByRole('option', { name: /Design Notes/ }));

    await waitFor(() => expect(openWorkspace).toHaveBeenCalledWith('project'));
    expect(useNavigationStore.getState().currentPage).toBe('chat');
    expect(onClose).toHaveBeenCalledOnce();
    expect(screen.queryByText('project-main')).not.toBeInTheDocument();
  });

  it('filters commands and supports keyboard navigation', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<CommandPalette isOpen onClose={onClose} />);

    const search = screen.getByRole('textbox', { name: '快速切换' });
    await user.type(search, '自动化{Enter}');

    expect(useNavigationStore.getState().currentPage).toBe('tasks');
    expect(onClose).toHaveBeenCalledOnce();
  });

  it.each(['superseded', 'navigated', 'reopened'] as const)('does not let a %s workspace command navigate or close a newer palette', async (mode) => {
    const user = userEvent.setup();
    const close = vi.fn();
    let finish!: (accepted: boolean) => void;
    openWorkspace.mockReturnValueOnce(new Promise<boolean>((resolve) => { finish = resolve; }));
    useNavigationStore.getState().setPage('memory');
    const frame = render(<CommandPalette isOpen onClose={close} />);
    await user.click(screen.getByRole('option', { name: /Design Notes/ }));
    if (mode === 'navigated') act(() => useNavigationStore.getState().setPage('knowledge'));
    if (mode === 'reopened') {
      frame.rerender(<CommandPalette isOpen={false} onClose={close} />);
      frame.rerender(<CommandPalette isOpen onClose={close} />);
    }
    await act(async () => { finish(mode !== 'superseded'); });
    expect(useNavigationStore.getState().currentPage).toBe(mode === 'navigated' ? 'knowledge' : 'memory');
    expect(close).not.toHaveBeenCalled();
  });

  it('lets reselecting the current workspace supersede a pending project opening through the real store', async () => {
    const user = userEvent.setup();
    const close = vi.fn();
    const requests: string[] = [];
    let finish!: (response: Response) => void;
    const pending = new Promise<Response>((resolve) => { finish = resolve; });
    const payload = (workspace: typeof personal | typeof project) => ({
      workspace, session: { id: workspace.activeSessionId, title: workspace.name, createdAt: 1, updatedAt: 1, contextVersion: 0 },
    });
    const fetch = vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url, init) => {
      const { cmd, args } = JSON.parse(String(init?.body));
      expect(cmd).toBe('open_workspace');
      requests.push(args.id);
      return args.id === project.id ? pending : new Response(JSON.stringify({ ok: true, data: payload(personal) }));
    });
    useWorkspacesStore.setState({ openWorkspace: realOpenWorkspace });
    useNavigationStore.getState().setPage('memory');
    const frame = render(<CommandPalette isOpen onClose={close} />);
    try {
      await user.click(screen.getByRole('option', { name: /Design Notes/ }));
      frame.rerender(<CommandPalette isOpen={false} onClose={close} />);
      frame.rerender(<CommandPalette isOpen onClose={close} />);
      await user.click(screen.getByRole('option', { name: /AngelBot 日常/ }));
      await waitFor(() => expect(requests).toEqual([project.id, personal.id]));
      await act(async () => { finish(new Response(JSON.stringify({ ok: true, data: payload(project) }))); });
      expect(useWorkspacesStore.getState().activeWorkspaceId).toBe(personal.id);
      expect(useNavigationStore.getState().currentPage).toBe('chat');
      expect(close).toHaveBeenCalledTimes(1);
    } finally { fetch.mockRestore(); }
  });

  it('provides a useful no-results state', async () => {
    const user = userEvent.setup();
    render(<CommandPalette isOpen onClose={vi.fn()} />);

    await user.type(screen.getByRole('textbox', { name: '快速切换' }), '不存在的入口');

    expect(screen.getByText('没有找到“不存在的入口”')).toBeInTheDocument();
    expect(screen.getByText(/试试项目名称/)).toBeInTheDocument();
  });
});
