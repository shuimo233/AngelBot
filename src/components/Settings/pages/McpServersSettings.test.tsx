import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { McpServersSettings } from './McpServersSettings';
import {
  clearMcpEnv,
  deleteMcpServer,
  getMcpServerStatus,
  getMcpServers,
  removeMcpEnvVar,
  refreshMcpTools,
  saveMcpServer,
  setMcpEnvVar,
  startMcpServer,
  stopMcpServer,
} from '$lib/commands/mcp';
import { getWorkspaces } from '$lib/commands/workspace';
import type { McpServer } from '$types';

vi.mock('$lib/commands/mcp', () => ({
  clearMcpEnv: vi.fn(),
  getMcpServers: vi.fn(),
  removeMcpEnvVar: vi.fn(),
  saveMcpServer: vi.fn(),
  setMcpEnvVar: vi.fn(),
  deleteMcpServer: vi.fn(),
  getMcpServerStatus: vi.fn(),
  refreshMcpTools: vi.fn(),
  startMcpServer: vi.fn(),
  stopMcpServer: vi.fn(),
}));
vi.mock('$lib/commands/workspace', () => ({ getWorkspaces: vi.fn() }));

const server = { id: 'filesystem', name: 'Filesystem', command: 'npx', args: '@modelcontextprotocol/server-filesystem', env: '', envKeys: [], envUnavailable: false, enabled: true, enabledWorkspaceIds: [] };
const stopped = { server_id: 'filesystem', status: 'stopped' as const, error: null, tools_count: null };
const running = { server_id: 'filesystem', status: 'running' as const, error: null, tools_count: 1 };

describe('McpServersSettings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getMcpServers).mockResolvedValue([server]);
    vi.mocked(getMcpServerStatus).mockResolvedValue(stopped);
    vi.mocked(saveMcpServer).mockResolvedValue();
    vi.mocked(clearMcpEnv).mockResolvedValue();
    vi.mocked(setMcpEnvVar).mockResolvedValue();
    vi.mocked(removeMcpEnvVar).mockResolvedValue();
    vi.mocked(deleteMcpServer).mockResolvedValue();
    vi.mocked(startMcpServer).mockResolvedValue(running);
    vi.mocked(stopMcpServer).mockResolvedValue(stopped);
    vi.mocked(refreshMcpTools).mockResolvedValue([{ name: 'list_files', description: 'List files', inputSchema: {} }]);
    vi.mocked(getWorkspaces).mockResolvedValue([{ id: 'personal', name: 'AngelBot 日常', kind: 'personal', rootPath: '', createdAt: 1, updatedAt: 1, activeSessionId: 'personal-main' }]);
  });

  it('loads configuration and keeps a stopped runtime separate from workspace permission', async () => {
    render(<McpServersSettings />);

    expect(await screen.findByText('Filesystem')).toBeInTheDocument();
    expect(screen.getByText('未连接')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'AngelBot 日常' })).not.toBeChecked();
    expect(screen.getByRole('checkbox', { name: '启用 Filesystem' })).toBeChecked();
    expect(screen.getByText(/MCP 服务不会自动继承 AngelBot 的模型密钥/)).toBeInTheDocument();
    expect(screen.getByText(/请勿将密钥写入命令或参数/)).toBeInTheDocument();
    expect(screen.getByText(/首次连接可能下载并运行第三方包/)).toBeInTheDocument();
    expect(screen.getByText('环境变量：尚未配置')).toBeInTheDocument();
  });

  it('guides Windows npx setup with a separate command and arguments', async () => {
    const user = userEvent.setup();
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('button', { name: '添加服务' }));

    expect(screen.getByPlaceholderText('npx.cmd')).toBeInTheDocument();
    expect(screen.getByPlaceholderText('-y @modelcontextprotocol/server-...')).toBeInTheDocument();
    expect(screen.getByText('Windows 下使用 npx 时，命令填写 npx.cmd；-y 和包名填写在参数栏。')).toBeInTheDocument();
  });

  it('persists enablement changes', async () => {
    const user = userEvent.setup();
    render(<McpServersSettings />);

    await screen.findByText('Filesystem');
    await user.click(screen.getByRole('checkbox', { name: '启用 Filesystem' }));

    await waitFor(() => expect(saveMcpServer).toHaveBeenCalledWith({ ...server, enabled: false }));
  });

  it('scopes a server to an explicitly selected workspace', async () => {
    const user = userEvent.setup();
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('checkbox', { name: 'AngelBot 日常' }));

    await waitFor(() => expect(saveMcpServer).toHaveBeenCalledWith({ ...server, enabledWorkspaceIds: ['personal'] }));
  });

  it('edits an existing service while preserving name-only runtime and clearing changed command grants', async () => {
    const user = userEvent.setup();
    let savedServer: McpServer = { ...server, enabledWorkspaceIds: ['personal'] };
    vi.mocked(getMcpServers).mockImplementation(async () => [savedServer]);
    vi.mocked(getMcpServerStatus).mockResolvedValue(running);
    vi.mocked(saveMcpServer).mockImplementation(async (next) => {
      const configurationChanged = next.command !== savedServer.command || next.args !== savedServer.args;
      savedServer = { ...next, enabledWorkspaceIds: configurationChanged ? [] : next.enabledWorkspaceIds };
    });
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('button', { name: '编辑 Filesystem' }));
    await user.clear(screen.getByRole('textbox', { name: '名称' }));
    await user.type(screen.getByRole('textbox', { name: '名称' }), 'File Tools');
    await user.click(screen.getByRole('button', { name: '保存修改' }));

    expect(await screen.findByRole('button', { name: '编辑 File Tools' })).toBeInTheDocument();
    expect(screen.getByText('运行中 · 待检查')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'AngelBot 日常' })).toBeChecked();

    await user.click(screen.getByRole('button', { name: '编辑 File Tools' }));
    expect(screen.getByText('修改命令或参数会停止服务并清除已有工作区授权；保存后需重新授权并连接检查。')).toBeInTheDocument();
    expect(screen.getByText('路径包含空格时可用英文双引号包裹。')).toBeInTheDocument();
    await user.type(screen.getByRole('textbox', { name: '参数' }), ' "D:\\My Files"');
    await user.click(screen.getByRole('button', { name: '保存修改' }));

    expect(await screen.findByText('未连接')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'AngelBot 日常' })).not.toBeChecked();
    expect(saveMcpServer).toHaveBeenLastCalledWith({ ...server, name: 'File Tools', args: `${server.args} "D:\\My Files"`, enabledWorkspaceIds: ['personal'] });
  });

  it('saves a new environment value once without echoing it, then requires reauthorization', async () => {
    const user = userEvent.setup();
    vi.mocked(getMcpServers).mockResolvedValue([{ ...server, enabledWorkspaceIds: ['personal'] }]);
    vi.mocked(getMcpServerStatus).mockResolvedValue(running);
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('button', { name: '配置 Filesystem 的环境变量' }));
    await user.type(screen.getByRole('textbox', { name: '变量名' }), 'SERVICE_TOKEN');
    const value = screen.getByLabelText('新变量值') as HTMLInputElement;
    expect(value.type).toBe('password');
    await user.type(value, 'secret-once');
    await user.click(screen.getByRole('button', { name: '保存变量' }));

    await waitFor(() => expect(setMcpEnvVar).toHaveBeenCalledWith('filesystem', 'SERVICE_TOKEN', 'secret-once'));
    expect(value).toHaveValue('');
    expect(screen.getByText('SERVICE_TOKEN')).toBeInTheDocument();
    expect(screen.getByText('已安全保存')).toBeInTheDocument();
    expect(screen.queryByText('secret-once')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '完成' }));
    expect(screen.getByText('未连接')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'AngelBot 日常' })).not.toBeChecked();
  });

  it('lists configured keys without preloading values and replaces only after a fresh entry', async () => {
    const user = userEvent.setup();
    vi.mocked(getMcpServers).mockResolvedValue([{ ...server, envKeys: ['SERVICE_TOKEN'] }]);
    render(<McpServersSettings />);

    expect(await screen.findByText('环境变量：SERVICE_TOKEN（已安全保存）')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '配置 Filesystem 的环境变量' }));
    await user.click(screen.getByRole('button', { name: '替换环境变量 SERVICE_TOKEN' }));
    expect(screen.getByRole('textbox', { name: '变量名' })).toHaveValue('SERVICE_TOKEN');
    expect(screen.getByLabelText('新变量值')).toHaveValue('');
    expect(screen.getByRole('button', { name: '保存变量' })).toBeDisabled();
    expect(setMcpEnvVar).not.toHaveBeenCalled();
  });

  it('confirms removal of an environment key and clears its workspace grants', async () => {
    const user = userEvent.setup();
    vi.mocked(getMcpServers).mockResolvedValue([{ ...server, envKeys: ['SERVICE_TOKEN'], enabledWorkspaceIds: ['personal'] }]);
    vi.mocked(getMcpServerStatus).mockResolvedValue(running);
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('button', { name: '配置 Filesystem 的环境变量' }));
    await user.click(screen.getByRole('button', { name: '移除环境变量 SERVICE_TOKEN' }));
    expect(removeMcpEnvVar).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: '确认移除环境变量 SERVICE_TOKEN' }));
    await waitFor(() => expect(removeMcpEnvVar).toHaveBeenCalledWith('filesystem', 'SERVICE_TOKEN'));
    expect(screen.getByText('尚未配置环境变量。')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '完成' }));
    expect(screen.getByText('未连接')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'AngelBot 日常' })).not.toBeChecked();
  });

  it('does not display a credential value returned inside a save error', async () => {
    const user = userEvent.setup();
    vi.mocked(setMcpEnvVar).mockRejectedValue(new Error('secret-once rejected'));
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('button', { name: '配置 Filesystem 的环境变量' }));
    await user.type(screen.getByRole('textbox', { name: '变量名' }), 'SERVICE_TOKEN');
    await user.type(screen.getByLabelText('新变量值'), 'secret-once');
    await user.click(screen.getByRole('button', { name: '保存变量' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('保存环境变量失败');
    expect(screen.getByLabelText('新变量值')).toHaveValue('');
    expect(screen.getByRole('alert')).not.toHaveTextContent('secret-once');
  });

  it('shows unavailable credentials and resets them only after explicit confirmation', async () => {
    const user = userEvent.setup();
    vi.mocked(getMcpServers).mockResolvedValue([{ ...server, envUnavailable: true, enabledWorkspaceIds: ['personal'] }]);
    render(<McpServersSettings />);

    expect(await screen.findByText('凭据不可用')).toBeInTheDocument();
    expect(screen.getByText('环境变量：凭据不可用')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '连接并检查' })).toBeDisabled();
    expect(screen.getByRole('checkbox', { name: 'AngelBot 日常' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: '修复 Filesystem 的环境变量' }));
    expect(screen.getByRole('alert')).toHaveTextContent('无法读取此服务在系统凭据库中的环境变量');
    expect(screen.queryByRole('textbox', { name: '变量名' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '重置凭据' }));
    expect(clearMcpEnv).not.toHaveBeenCalled();
    expect(screen.getByText(/旧值无法从此处找回/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '确认重置凭据' }));

    await waitFor(() => expect(clearMcpEnv).toHaveBeenCalledWith('filesystem'));
    expect(screen.getByText('尚未配置环境变量。')).toBeInTheDocument();
    expect(screen.getByLabelText('新变量值')).toHaveValue('');
    await user.click(screen.getByRole('button', { name: '完成' }));
    expect(screen.getByText('未连接')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'AngelBot 日常' })).not.toBeChecked();
  });

  it('keeps recovery unavailable when resetting fails without exposing the error body', async () => {
    const user = userEvent.setup();
    vi.mocked(getMcpServers).mockResolvedValue([{ ...server, envUnavailable: true }]);
    vi.mocked(clearMcpEnv).mockRejectedValue(new Error('secret-once from credential store'));
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('button', { name: '修复 Filesystem 的环境变量' }));
    await user.click(screen.getByRole('button', { name: '重置凭据' }));
    await user.click(screen.getByRole('button', { name: '确认重置凭据' }));

    expect(await screen.findByText('重置 MCP 凭据失败，请检查系统凭据库后重试。')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '确认重置凭据' })).toBeInTheDocument();
    expect(screen.queryByText(/secret-once/)).not.toBeInTheDocument();
  });

  it('explicitly starts, freshly discovers, and presents only tool names', async () => {
    const user = userEvent.setup();
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('button', { name: '连接并检查' }));

    await waitFor(() => expect(startMcpServer).toHaveBeenCalledWith('filesystem'));
    expect(refreshMcpTools).toHaveBeenCalledWith('filesystem');
    expect(await screen.findByText('已发现 1 个工具；下次发送已授权工作区的消息时可供 AngelBot 使用。')).toBeInTheDocument();
    expect(screen.getByText('list_files')).toBeInTheDocument();
    expect(saveMcpServer).not.toHaveBeenCalled();
  });

  it('refreshes a running service without starting it twice and can stop it', async () => {
    const user = userEvent.setup();
    vi.mocked(getMcpServerStatus).mockResolvedValue(running);
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('button', { name: '重新检查' }));

    await waitFor(() => expect(refreshMcpTools).toHaveBeenCalledWith('filesystem'));
    expect(startMcpServer).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: '停止' }));
    await waitFor(() => expect(stopMcpServer).toHaveBeenCalledWith('filesystem'));
    expect(await screen.findByText('未连接')).toBeInTheDocument();
  });

  it('does not claim a connection when startup fails', async () => {
    const user = userEvent.setup();
    vi.mocked(startMcpServer).mockRejectedValue(new Error('secret-once in child error'));
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('button', { name: '连接并检查' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('无法连接“Filesystem”');
    expect(screen.getByRole('alert')).not.toHaveTextContent('secret-once');
    expect(screen.getByText('连接失败')).toBeInTheDocument();
    expect(screen.queryByText(/已发现 1 个工具/)).not.toBeInTheDocument();
  });

  it('explains an npx first-connect timeout without echoing transport output', async () => {
    const user = userEvent.setup();
    vi.mocked(startMcpServer).mockRejectedValue(new Error('MCP initialize failed: MCP server response timed out secret-once'));
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('button', { name: '连接并检查' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('首次运行可能需要下载依赖');
    expect(screen.getByRole('alert')).toHaveTextContent('检查网络和包名后重试');
    expect(screen.queryByText(/secret-once/)).not.toBeInTheDocument();
  });

  it('explains a missing launcher without showing the command or OS error', async () => {
    const user = userEvent.setup();
    vi.mocked(startMcpServer).mockRejectedValue(new Error('Failed to start MCP server (command: secret-once): path not found'));
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('button', { name: '连接并检查' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('无法启动命令');
    expect(screen.queryByText(/secret-once/)).not.toBeInTheDocument();
  });

  it('shows runtime errors returned by the server status', async () => {
    vi.mocked(getMcpServerStatus).mockResolvedValue({ ...stopped, status: 'error', error: 'secret-once in status' });
    render(<McpServersSettings />);

    expect(await screen.findByText('连接失败')).toBeInTheDocument();
    expect(screen.getByText('连接错误；请检查服务配置和本机依赖。')).toBeInTheDocument();
    expect(screen.queryByText(/secret-once/)).not.toBeInTheDocument();
  });

  it('shows stopping state until the stop request finishes', async () => {
    const user = userEvent.setup();
    let finishStop!: (status: typeof stopped) => void;
    vi.mocked(getMcpServerStatus).mockResolvedValue(running);
    vi.mocked(stopMcpServer).mockReturnValue(new Promise((resolve) => { finishStop = resolve; }));
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('button', { name: '停止' }));
    expect(screen.getByText('正在停止')).toBeInTheDocument();
    finishStop(stopped);
    expect(await screen.findByText('未连接')).toBeInTheDocument();
  });

  it('can cancel an in-flight connection without reviving a stale check result', async () => {
    const user = userEvent.setup();
    const starting = { ...stopped, status: 'starting' as const };
    let startRequested = false;
    let stoppedByUser = false;
    let finishStart!: (status: typeof running) => void;
    vi.mocked(getMcpServerStatus).mockImplementation(async () => (
      stoppedByUser ? stopped : startRequested ? starting : stopped
    ));
    vi.mocked(startMcpServer).mockImplementation(() => {
      startRequested = true;
      return new Promise((resolve) => { finishStart = resolve; });
    });
    vi.mocked(stopMcpServer).mockImplementation(async () => {
      stoppedByUser = true;
      return stopped;
    });
    render(<McpServersSettings />);

    await user.click(await screen.findByRole('button', { name: '连接并检查' }));
    expect(screen.getByText('正在连接')).toBeInTheDocument();
    await user.click(await screen.findByRole('button', { name: '取消连接' }));

    await waitFor(() => expect(stopMcpServer).toHaveBeenCalledWith('filesystem'));
    expect(await screen.findByText('未连接')).toBeInTheDocument();
    await act(async () => { finishStart(running); });
    expect(screen.getByText('未连接')).toBeInTheDocument();
    expect(refreshMcpTools).not.toHaveBeenCalled();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});
