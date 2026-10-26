import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DataSettings } from './DataSettings';
import { useWorkspacesStore } from '$stores/workspaces';
import { confirmRestoredProjectDirectories } from '$lib/commands/workspace';

vi.mock('$lib/commands/workspace', async (original) => ({ ...await original<typeof import('$lib/commands/workspace')>(), confirmRestoredProjectDirectories: vi.fn() }));

describe('DataSettings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useWorkspacesStore.setState({ workspaces: [], lastError: null, loadWorkspaces: vi.fn(async () => {}) });
  });
  it('distinguishes ordinary and encrypted MCP credential backups without offering to reveal values', async () => {
    const user = userEvent.setup();
    render(<DataSettings />);
    await user.click(screen.getByRole('button', { name: '数据备份' }));

    expect(screen.getByText(/普通备份不包含 MCP 专用密钥/)).toBeInTheDocument();
    expect(screen.getByText(/加密备份可携带这些密钥/)).toBeInTheDocument();
    expect(screen.getByText(/恢复时会写入系统凭据库，不会回显原值/)).toBeInTheDocument();
    expect(screen.getByText(/新版备份保留项目、主对话、会话分支及主 Agent 工具回执/)).toBeInTheDocument();
    expect(screen.getByText(/不包含项目文件原件、子代理工作树或自动化执行队列/)).toBeInTheDocument();
  });

  it('keeps history available and submits one explicit directory batch, retaining unchecked projects', async () => {
    const user = userEvent.setup();
    useWorkspacesStore.setState({ workspaces: [
      { id: 'one', name: '资料项目', kind: 'project', rootPath: 'D:/old/one', directoryConfirmed: false, createdAt: 1, updatedAt: 1, activeSessionId: 's1' },
      { id: 'two', name: '另一个项目', kind: 'project', rootPath: 'D:/old/two', directoryConfirmed: false, createdAt: 1, updatedAt: 1, activeSessionId: 's2' },
    ] });
    vi.mocked(confirmRestoredProjectDirectories).mockRejectedValueOnce(new Error('项目目录不存在'));
    render(<DataSettings />);
    expect(confirmRestoredProjectDirectories).not.toHaveBeenCalled();
    expect(screen.getByText(/历史已经恢复，目录尚未授权/)).toBeInTheDocument();
    await user.click(screen.getByRole('checkbox', { name: '另一个项目' }));
    await user.click(screen.getByRole('button', { name: '确认选中的 1 个目录' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('项目目录不存在');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(confirmRestoredProjectDirectories).toHaveBeenCalledTimes(1);
    expect(confirmRestoredProjectDirectories).toHaveBeenCalledWith([{ id: 'one', expectedPath: 'D:/old/one', path: 'D:/old/one' }]);
    expect(useWorkspacesStore.getState().loadWorkspaces).not.toHaveBeenCalled();
    vi.mocked(confirmRestoredProjectDirectories).mockResolvedValueOnce(undefined);
    await user.click(screen.getByRole('button', { name: '确认选中的 1 个目录' }));
    expect(useWorkspacesStore.getState().loadWorkspaces).toHaveBeenCalledOnce();
    expect(screen.getByRole('status')).toHaveTextContent('已确认 1 个项目目录');
  });
});
