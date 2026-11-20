import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { ProjectsPage } from './ProjectsPage';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';

beforeEach(() => {
  useNavigationStore.setState({ currentPage: 'projects' });
  useWorkspacesStore.setState({ loading: false, lastError: null, workspaces: [
    { id: 'project', name: '旅行计划', kind: 'project', rootPath: 'D:/Travel', activeSessionId: 'existing-main', createdAt: 1, updatedAt: 1, directoryConfirmed: false },
  ], openWorkspace: vi.fn().mockResolvedValue(true) });
});

it('opens the existing project Main without granting or creating a directory', async () => {
  const user = userEvent.setup();
  render(<ProjectsPage />);
  expect(screen.queryByRole('searchbox', { name: '搜索项目' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: /旅行计划.*目录待确认/ }));
  expect(useWorkspacesStore.getState().openWorkspace).toHaveBeenCalledTimes(1);
  expect(useWorkspacesStore.getState().openWorkspace).toHaveBeenCalledWith('project');
  expect(useNavigationStore.getState().currentPage).toBe('chat');
});

it('filters the full catalog and restores a short list when the search is cleared', async () => {
  const user = userEvent.setup();
  const workspace = useWorkspacesStore.getState().workspaces[0];
  useWorkspacesStore.setState({ workspaces: Array.from({ length: 25 }, (_, index) => ({
    ...workspace,
    id: `project-${index + 1}`,
    name: index === 24 ? '远期旅程' : `项目 ${index + 1}`,
    rootPath: `D:\\Travel\\Trip-${index + 1}`,
  })) });
  render(<ProjectsPage />);

  expect(screen.queryByRole('button', { name: /远期旅程/ })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: '显示更多（还有 5 个）' }));
  expect(screen.getByRole('button', { name: /项目 21/ })).toHaveFocus();
  expect(screen.getByRole('button', { name: /远期旅程/ })).toBeVisible();

  const searchbox = screen.getByRole('searchbox', { name: '搜索项目' });
  await user.type(searchbox, 'd:/travel/trip-25');
  expect(screen.getByRole('status')).toHaveTextContent('找到 1 个项目');
  expect(screen.getByRole('button', { name: /远期旅程/ })).toBeVisible();
  expect(screen.queryByRole('button', { name: /项目 21/ })).not.toBeInTheDocument();
  await user.clear(searchbox);
  expect(screen.queryByRole('button', { name: /远期旅程/ })).not.toBeInTheDocument();
  await user.type(searchbox, '远期');
  const match = screen.getByRole('button', { name: /远期旅程/ });
  await user.click(match);
  expect(useWorkspacesStore.getState().openWorkspace).toHaveBeenCalledWith('project-25');
});

it.each(['late', 'error'] as const)('preserves navigation and actionable failures (%s)', async mode => {
  let finish!: (accepted: boolean) => void;
  const open = vi.fn(() => mode === 'error' ? Promise.reject(new Error('目录不可用')) : new Promise<boolean>(resolve => { finish = resolve; }));
  useWorkspacesStore.setState({ openWorkspace: open });
  const user = userEvent.setup();
  render(<ProjectsPage />);
  await user.click(screen.getByRole('button', { name: /旅行计划.*目录待确认/ }));
  if (mode === 'error') expect(await screen.findByRole('alert')).toHaveTextContent('目录不可用');
  else {
    act(() => useNavigationStore.getState().setPage('tasks'));
    await act(async () => { finish(true); });
    expect(useNavigationStore.getState().currentPage).toBe('tasks');
  }
});
