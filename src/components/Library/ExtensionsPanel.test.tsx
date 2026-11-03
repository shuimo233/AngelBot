import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { ExtensionsPanel } from './ExtensionsPanel';
import { getSkills } from '$lib/commands/skill';
import { getMcpServers, startMcpServer } from '$lib/commands/mcp';
import { useWorkspacesStore } from '$stores/workspaces';

vi.mock('$lib/commands/skill', () => ({ getSkills: vi.fn() }));
vi.mock('$lib/commands/mcp', () => ({ getMcpServers: vi.fn(), startMcpServer: vi.fn() }));
beforeEach(() => {
  vi.clearAllMocks();
  useWorkspacesStore.setState({ activeWorkspaceId: 'daily', workspaces: [{ id: 'daily', name: '日常', kind: 'personal', activeSessionId: 'main', createdAt: 1, updatedAt: 1 }] });
  vi.mocked(getSkills).mockResolvedValue([{ id: 'writer', name: '写作规范', description: '核对文档用语', version: '1.0', author: 'Example', dependencies: ['external-tool'], permissions: ['file-read'], actions: [], prompts: [{ name: '检查', content: '<script>不执行</script>' }] }]);
  vi.mocked(getMcpServers).mockResolvedValue([{ id: 'browser', name: '浏览器工具', enabled: true, enabledWorkspaceIds: ['daily'], catalogReady: true, envUnavailable: false, envKeys: ['SECRET'], command: 'npx.cmd', args: 'should-not-be-shown', env: '' }]);
});

it('shows actual metadata and compatibility limits without starting services or exposing arguments', async () => {
  const user = userEvent.setup();
  const frame = render(<ExtensionsPanel kind="skills" />);
  expect(await screen.findByText('写作规范')).toBeInTheDocument();
  expect(getMcpServers).not.toHaveBeenCalled();
  await user.click(screen.getByText('查看详情'));
  expect(screen.getByText('external-tool')).toBeInTheDocument();
  expect(screen.getByText(/声明不是授权/)).toBeInTheDocument();
  await user.click(screen.getByText('模板：检查'));
  expect(screen.getByText('<script>不执行</script>')).toBeInTheDocument();
  expect(frame.container.querySelector('script')).toBeNull();
  expect(screen.queryByText('should-not-be-shown')).not.toBeInTheDocument();
  expect(screen.queryByText('SECRET')).not.toBeInTheDocument();
  await user.type(screen.getByRole('searchbox'), 'external-tool');
  expect(screen.getByText('写作规范')).toBeInTheDocument();
  expect(screen.queryByText('浏览器工具')).not.toBeInTheDocument();
  expect(startMcpServer).not.toHaveBeenCalled();
  expect(getMcpServers).not.toHaveBeenCalled();
});

it('keeps a skill read failure distinct without loading connections', async () => {
  vi.mocked(getSkills).mockRejectedValueOnce(new Error('failed'));
  const user = userEvent.setup();
    render(<ExtensionsPanel kind="skills" />);
    expect(await screen.findByRole('alert')).toHaveTextContent('无法加载已添加的技能');
    expect(screen.queryByText('还没有已添加的技能')).not.toBeInTheDocument();
    expect(getMcpServers).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: '重新加载' }));
    await waitFor(() => expect(screen.getByText('写作规范')).toBeInTheDocument());
    expect(startMcpServer).not.toHaveBeenCalled();
});

it('keeps connection configuration separate from skill installation and never starts a service', async () => {
  const settings = vi.fn();
  window.addEventListener('open-settings', settings);
  const user = userEvent.setup();
  try {
    const frame = render(<ExtensionsPanel kind="connections" />);
    expect(await screen.findByText('已设置 · 按需连接')).toBeInTheDocument();
    expect(getSkills).not.toHaveBeenCalled();
    expect(screen.queryByText('如何添加技能')).not.toBeInTheDocument();
    expect(frame.container.textContent).not.toContain('should-not-be-shown');
    expect(frame.container.textContent).not.toContain('SECRET');
    const help = screen.getByText('连接说明');
    expect(help.closest('details')).not.toHaveAttribute('open');
    await user.click(help);
    expect(help.closest('details')).toHaveAttribute('open');
    expect(screen.getByText('接入方式')).toBeVisible();
    expect(screen.getByText('运行状态')).toBeVisible();
    expect(screen.getByText('操作权限')).toBeVisible();
    expect(screen.getByText(/保存配置不代表服务正在运行/)).toBeVisible();
    expect(screen.getByText(/已有配置不代表已授权所有操作/)).toBeVisible();
    await user.click(screen.getByRole('button', { name: '管理连接' }));
    expect(settings.mock.calls[0][0].detail).toBe('mcp');
    expect(startMcpServer).not.toHaveBeenCalled();
  } finally { window.removeEventListener('open-settings', settings); }
});
