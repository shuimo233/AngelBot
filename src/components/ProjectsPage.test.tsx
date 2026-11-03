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
  await user.click(screen.getByRole('button', { name: /旅行计划.*目录待确认/ }));
  expect(useWorkspacesStore.getState().openWorkspace).toHaveBeenCalledTimes(1);
  expect(useWorkspacesStore.getState().openWorkspace).toHaveBeenCalledWith('project');
  expect(useNavigationStore.getState().currentPage).toBe('chat');
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
