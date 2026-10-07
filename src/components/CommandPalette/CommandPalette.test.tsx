import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { CommandPalette } from './index';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';

const personal = { id: 'personal', name: 'AngelBot', kind: 'personal' as const, createdAt: 1, updatedAt: 2, activeSessionId: 'personal-main' };
const project = { id: 'project', name: 'Design Notes', kind: 'project' as const, rootPath: 'D:\\Projects\\Design Notes', createdAt: 1, updatedAt: 3, activeSessionId: 'project-main' };

describe('CommandPalette', () => {
  const openWorkspace = vi.fn().mockResolvedValue(undefined);

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
      createProject: vi.fn().mockResolvedValue(undefined),
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

  it('provides a useful no-results state', async () => {
    const user = userEvent.setup();
    render(<CommandPalette isOpen onClose={vi.fn()} />);

    await user.type(screen.getByRole('textbox', { name: '快速切换' }), '不存在的入口');

    expect(screen.getByText('没有找到“不存在的入口”')).toBeInTheDocument();
    expect(screen.getByText(/试试项目名称/)).toBeInTheDocument();
  });
});
