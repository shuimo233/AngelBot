import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getAgentExecutionPermission, setAgentExecutionPermission, type AgentExecutionPermission } from '$lib/commands/settings';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspacesStore } from '$stores/workspaces';
import { useExecutionPermissionStore } from '$stores/executionPermission';
import { ExecutionPermissionSettings } from './ExecutionPermissionSettings';

vi.mock('$lib/commands/settings', () => ({
  getAgentExecutionPermission: vi.fn(),
  setAgentExecutionPermission: vi.fn(),
}));

describe('ExecutionPermissionSettings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useSessionsStore.setState({ sessions: [], activeSessionId: null, activeSession: null });
    useWorkspacesStore.setState({ workspaces: [], activeWorkspaceId: null, loading: false, lastError: null });
    useExecutionPermissionStore.setState({ permission: null, loading: true, saving: false, error: null });
    vi.mocked(getAgentExecutionPermission).mockResolvedValue('ask');
    vi.mocked(setAgentExecutionPermission).mockImplementation(async (permission) => permission);
  });

  it('loads and saves the global permission without an active workspace', async () => {
    const user = userEvent.setup();
    render(<ExecutionPermissionSettings />);

    await waitFor(() => expect(getAgentExecutionPermission).toHaveBeenCalledOnce());
    expect(screen.getByText(/适用于所有工作区/)).toHaveTextContent('各自授权照常生效');
    expect(await screen.findByText(/普通 MCP 发送或删除可能直接执行/)).toHaveTextContent('仍需单独确认');

    await user.click(screen.getByRole('button', { name: /完全访问/ }));

    await waitFor(() => expect(setAgentExecutionPermission).toHaveBeenCalledWith('full_access'));
    expect(screen.getByRole('button', { name: '完全访问', pressed: true })).toBeInTheDocument();
  });

  it('restores the previous selection if saving fails', async () => {
    const user = userEvent.setup();
    vi.mocked(setAgentExecutionPermission).mockRejectedValue(new Error('保存失败'));
    render(<ExecutionPermissionSettings />);

    await waitFor(() => expect(getAgentExecutionPermission).toHaveBeenCalledOnce());
    await user.click(screen.getByRole('button', { name: /工作区自动/ }));

    expect(await screen.findByRole('alert')).toHaveTextContent('保存失败');
    expect(screen.getByRole('button', { name: '请求批准', pressed: true })).toBeInTheDocument();
  });

  it('does not show a safer default while the persisted permission is loading', async () => {
    let finishRead!: (permission: AgentExecutionPermission) => void;
    vi.mocked(getAgentExecutionPermission).mockReturnValue(new Promise((resolve) => { finishRead = resolve; }));
    render(<ExecutionPermissionSettings />);

    expect(screen.getByText('正在读取当前执行权限…')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '请求批准', pressed: true })).not.toBeInTheDocument();
    await act(async () => finishRead('full_access'));
    expect(screen.getByRole('button', { name: '完全访问', pressed: true })).toBeInTheDocument();
  });

  it('shows only the committed mode and serializes permission changes', async () => {
    let finishSave!: (permission: AgentExecutionPermission) => void;
    vi.mocked(getAgentExecutionPermission).mockResolvedValue('full_access');
    vi.mocked(setAgentExecutionPermission).mockReturnValue(new Promise((resolve) => { finishSave = resolve; }));
    const user = userEvent.setup();
    render(<ExecutionPermissionSettings />);

    await user.click(await screen.findByRole('button', { name: '请求批准' }));
    expect(screen.getByRole('button', { name: '完全访问', pressed: true })).toBeInTheDocument();
    expect(screen.getByText(/保存完成前仍按原设置运行/)).toBeInTheDocument();
    const other = screen.getByRole('button', { name: '工作区自动' });
    expect(other).toHaveAttribute('aria-disabled', 'true');
    fireEvent.click(other);
    expect(setAgentExecutionPermission).toHaveBeenCalledTimes(1);
    await act(async () => finishSave('ask'));
    expect(screen.getByRole('button', { name: '请求批准', pressed: true })).toHaveFocus();
  });
});
