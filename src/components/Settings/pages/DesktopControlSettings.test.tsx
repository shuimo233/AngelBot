import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { open } from '@tauri-apps/plugin-dialog';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  confirmDesktopObservationScope,
  getDesktopTrustedAppStatuses,
  getDesktopTrustedApps,
  inspectDesktopDraftTargets,
  saveDesktopTrustedApp,
  type TrustedDesktopApp,
} from '$lib/commands/desktop';
import { DesktopControlSettings } from './DesktopControlSettings';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));
vi.mock('$lib/commands/desktop', () => ({
  confirmDesktopObservationScope: vi.fn(),
  getDesktopTrustedApps: vi.fn(),
  getDesktopTrustedAppStatuses: vi.fn(),
  inspectDesktopDraftTargets: vi.fn(),
  saveDesktopTrustedApp: vi.fn(),
  deleteDesktopTrustedApp: vi.fn(),
}));

describe('DesktopControlSettings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(confirmDesktopObservationScope).mockReset();
    vi.mocked(getDesktopTrustedApps).mockResolvedValue([]);
    vi.mocked(getDesktopTrustedAppStatuses).mockResolvedValue([]);
    vi.mocked(open).mockReset();
  });

  const savedApp: TrustedDesktopApp = {
    id: 'chat',
    displayName: 'Chat',
    executablePath: 'C:\\Apps\\chat.exe',
    capabilities: ['launch', 'observe', 'observeImage'],
    draftSelector: null,
    enabled: false,
    createdAt: 1,
    updatedAt: 1,
  };

  it('uses the Windows file picker to configure a trusted application', async () => {
    vi.mocked(open).mockResolvedValue('C:\\Program Files\\Example App\\example.exe');
    const user = userEvent.setup();

    render(<DesktopControlSettings />);
    await waitFor(() => expect(getDesktopTrustedApps).toHaveBeenCalled());
    await user.click(screen.getByRole('button', { name: '选择程序' }));

    expect(open).toHaveBeenCalledWith(expect.objectContaining({
      directory: false,
      multiple: false,
    }));
    expect(screen.getByLabelText('程序路径')).toHaveValue(
      'C:\\Program Files\\Example App\\example.exe',
    );
    expect(screen.getByLabelText('显示名称')).toHaveValue('example');
    expect(screen.getByLabelText(/内部标识/)).toHaveValue('example');
  });

  it('shows when a configured application must be repaired', async () => {
    vi.mocked(getDesktopTrustedApps).mockResolvedValue([{
      id: 'chat',
      displayName: 'Chat',
      executablePath: 'C:\\Missing\\chat.exe',
      capabilities: ['launch'],
      draftSelector: null,
      enabled: true,
      createdAt: 1,
      updatedAt: 1,
    }]);
    vi.mocked(getDesktopTrustedAppStatuses).mockResolvedValue([{
      id: 'chat',
      status: 'executableUnavailable',
    }]);

    render(<DesktopControlSettings />);

    expect(await screen.findByText((content, element) => (
      element?.tagName === 'SMALL'
      && content.includes('程序不可用，请重新选择')
      && content.includes('打开应用')
    ))).toBeInTheDocument();
  });

  it('allows a view-only app without a separate observation checkbox', async () => {
    const user = userEvent.setup();
    render(<DesktopControlSettings />);
    await waitFor(() => expect(getDesktopTrustedApps).toHaveBeenCalled());
    await user.type(screen.getByLabelText('程序路径'), 'C:\\Apps\\viewer.exe');
    await user.type(screen.getByLabelText('显示名称'), 'Viewer');
    await user.type(screen.getByLabelText(/内部标识/), 'viewer');
    await user.click(screen.getByLabelText('允许打开应用'));
    expect(screen.queryByLabelText(/允许 AngelBot 查看应用当前页面/)).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '添加并允许查看' }));
    await waitFor(() => expect(saveDesktopTrustedApp).toHaveBeenCalledWith(expect.objectContaining({
      id: 'viewer',
      capabilities: [],
      draftSelector: null,
    })));
  });

  it('keeps generic text filling off until the user grants it for an app', async () => {
    const user = userEvent.setup();
    render(<DesktopControlSettings />);
    await waitFor(() => expect(getDesktopTrustedApps).toHaveBeenCalled());

    expect(screen.getByLabelText('允许填写普通文本字段')).not.toBeChecked();
    await user.type(screen.getByLabelText('程序路径'), 'C:\\Apps\\editor.exe');
    await user.type(screen.getByLabelText('显示名称'), 'Editor');
    await user.type(screen.getByLabelText(/内部标识/), 'editor');
    await user.click(screen.getByLabelText('允许填写普通文本字段'));
    await user.click(screen.getByRole('button', { name: '添加并允许查看' }));

    await waitFor(() => expect(saveDesktopTrustedApp).toHaveBeenCalledWith(expect.objectContaining({
      id: 'editor',
      capabilities: ['launch', 'fill'],
      draftSelector: null,
    })));
  });

  it('keeps button interaction off for new and existing apps until explicitly granted', async () => {
    vi.mocked(getDesktopTrustedApps).mockResolvedValue([savedApp]);
    vi.mocked(saveDesktopTrustedApp).mockResolvedValue(savedApp);
    const user = userEvent.setup();
    render(<DesktopControlSettings />);

    expect(screen.getByLabelText('操作控件')).not.toBeChecked();
    await user.click(await screen.findByRole('button', { name: '编辑' }));
    expect(screen.getByLabelText('操作控件')).not.toBeChecked();
    await user.click(screen.getByRole('button', { name: '保存配置' }));
    await waitFor(() => expect(saveDesktopTrustedApp).toHaveBeenCalledWith(expect.objectContaining({
      id: 'chat', capabilities: ['launch'],
    })));

    await user.click(await screen.findByRole('button', { name: '编辑' }));
    await user.click(screen.getByLabelText('操作控件'));
    expect(screen.getByText(/允许后可逐次确认并调用、选中、展开或收起/)).toBeInTheDocument();
    expect(screen.getByText(/可能触发发送、删除或其他不可撤销后果/)).toBeInTheDocument();
    expect(screen.getByText(/其他回执仅确认控件状态，仍需检查任务结果/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '保存配置' }));
    await waitFor(() => expect(saveDesktopTrustedApp).toHaveBeenLastCalledWith(expect.objectContaining({
      id: 'chat', capabilities: ['launch', 'interact'],
    })));
    expect(screen.getByLabelText('操作控件')).not.toBeChecked();
  });

  it('loads an existing explicit interaction grant and labels it distinctly from observation', async () => {
    vi.mocked(getDesktopTrustedApps).mockResolvedValue([{ ...savedApp, capabilities: ['observe', 'observeImage', 'interact'] }]);
    const user = userEvent.setup();
    render(<DesktopControlSettings />);
    expect(await screen.findByText((content, element) => element?.tagName === 'SMALL'
      && content.includes('可查看结构与图像 · 操作控件'))).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '编辑' }));
    expect(screen.getByLabelText('操作控件')).toBeChecked();
    expect(screen.getByLabelText('允许填写普通文本字段')).not.toBeChecked();
  });

  it('requires an explicit one-time confirmation for legacy app observation', async () => {
    const legacy = { ...savedApp, capabilities: ['launch'] as TrustedDesktopApp['capabilities'] };
    vi.mocked(getDesktopTrustedApps).mockResolvedValue([legacy]);
    const user = userEvent.setup();
    render(<DesktopControlSettings />);

    expect(await screen.findByText(/旧配置需要确认/)).toBeInTheDocument();
    expect(screen.getAllByText(/尚未允许查看/).length).toBeGreaterThan(0);
    expect(confirmDesktopObservationScope).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: '允许查看上述应用' }));
    await waitFor(() => expect(confirmDesktopObservationScope).toHaveBeenCalledWith([
      { id: 'chat', executablePath: 'C:\\Apps\\chat.exe' },
    ]));
    expect(saveDesktopTrustedApp).not.toHaveBeenCalled();
  });

  it('does not silently expand a structural observation grant to window images', async () => {
    vi.mocked(getDesktopTrustedApps).mockResolvedValue([{ ...savedApp, capabilities: ['launch', 'observe'] }]);
    const user = userEvent.setup();
    render(<DesktopControlSettings />);

    expect(await screen.findByText(/原有结构查看和操作权限保持不变/)).toHaveTextContent('可能包含私人内容');
    expect(screen.getByText(/仅结构，图像需确认/)).toBeInTheDocument();
    expect(confirmDesktopObservationScope).not.toHaveBeenCalled();
    expect(saveDesktopTrustedApp).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: '编辑' }));
    expect(screen.getByText(/保存后会允许将此应用的单窗口图像交给当前模型/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '保存并允许查看' })).toBeInTheDocument();
    expect(screen.getAllByRole('checkbox').every((element) => !element.parentElement?.textContent?.includes('图像'))).toBe(true);
  });

  it('keeps legacy scope unchanged and shows a nearby error when confirmation fails', async () => {
    vi.mocked(getDesktopTrustedApps).mockResolvedValue([{ ...savedApp, capabilities: ['launch'] }]);
    vi.mocked(confirmDesktopObservationScope).mockRejectedValueOnce('Application path changed');
    const user = userEvent.setup();
    render(<DesktopControlSettings />);

    await user.click(await screen.findByRole('button', { name: '允许查看上述应用' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('确认失败：Application path changed');
    expect(screen.getByRole('button', { name: '允许查看上述应用' })).toBeEnabled();
    expect(saveDesktopTrustedApp).not.toHaveBeenCalled();
  });

  it('discovers a target for a saved disabled app without granting permission until save', async () => {
    vi.mocked(getDesktopTrustedApps).mockResolvedValue([savedApp]);
    vi.mocked(getDesktopTrustedAppStatuses).mockResolvedValue([{ id: 'chat', status: 'disabled' }]);
    vi.mocked(inspectDesktopDraftTargets).mockResolvedValue({ appId: 'chat', candidates: [{ name: '消息输入框' }] });
    const user = userEvent.setup();

    render(<DesktopControlSettings />);
    await user.click(await screen.findByRole('button', { name: '编辑' }));
    await user.click(screen.getByLabelText('允许填写消息草稿'));
    await user.click(screen.getByRole('button', { name: '查找当前窗口的输入框' }));

    expect(inspectDesktopDraftTargets).toHaveBeenCalledWith('chat');
    expect(await screen.findByRole('button', { name: '选择“消息输入框”' })).toBeInTheDocument();
    expect(screen.getByLabelText(/草稿输入框名称/)).toHaveValue('');
    expect(saveDesktopTrustedApp).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: '选择“消息输入框”' }));
    expect(screen.getByLabelText(/草稿输入框名称/)).toHaveValue('消息输入框');
    expect(saveDesktopTrustedApp).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: '保存配置' }));
    await waitFor(() => expect(saveDesktopTrustedApp).toHaveBeenCalledWith(expect.objectContaining({
      id: 'chat',
      capabilities: ['launch', 'draft'],
      draftSelector: '消息输入框',
      enabled: false,
    })));
  });

  it('requires a saved app before discovering, and reports empty or failed scans', async () => {
    const user = userEvent.setup();
    const view = render(<DesktopControlSettings />);
    await user.click(screen.getByLabelText('允许填写消息草稿'));
    expect(screen.getByRole('button', { name: '查找当前窗口的输入框' })).toBeDisabled();
    expect(screen.getByText(/新应用请先只勾选/)).toBeInTheDocument();

    vi.mocked(getDesktopTrustedApps).mockResolvedValue([savedApp]);
    vi.mocked(inspectDesktopDraftTargets).mockResolvedValueOnce({ appId: 'chat', candidates: [] });
    view.unmount();
    render(<DesktopControlSettings />);
    await user.click(await screen.findByRole('button', { name: '编辑' }));
    await user.click(screen.getByLabelText('允许填写消息草稿'));
    await user.click(screen.getByRole('button', { name: '查找当前窗口的输入框' }));
    expect(await screen.findByText(/没有可安全填写的输入框/)).toBeInTheDocument();

    vi.mocked(inspectDesktopDraftTargets).mockRejectedValueOnce('TARGET_AMBIGUOUS: The trusted application has multiple main windows');
    await user.click(screen.getByRole('button', { name: '查找当前窗口的输入框' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('请只保留需要填写的窗口后重试');
    vi.mocked(inspectDesktopDraftTargets).mockRejectedValueOnce('SCAN_LIMIT: Too many edit controls were found');
    await user.click(screen.getByRole('button', { name: '查找当前窗口的输入框' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('切换到输入框较少的页面后重试');
    expect(saveDesktopTrustedApp).not.toHaveBeenCalled();
  });

  it('flags an old selector and clears results when switching to another saved app', async () => {
    vi.mocked(getDesktopTrustedApps).mockResolvedValue([
      { ...savedApp, capabilities: ['launch', 'observe', 'observeImage', 'draft'], draftSelector: '旧输入框' },
      { ...savedApp, id: 'editor', displayName: 'Editor', executablePath: 'C:\\Apps\\editor.exe' },
    ]);
    vi.mocked(inspectDesktopDraftTargets).mockResolvedValue({ appId: 'chat', candidates: [{ name: '新输入框' }] });
    const user = userEvent.setup();

    render(<DesktopControlSettings />);
    const editButtons = await screen.findAllByRole('button', { name: '编辑' });
    await user.click(editButtons[0]);
    await user.click(screen.getByRole('button', { name: '查找当前窗口的输入框' }));
    expect(await screen.findByText(/当前填写的名称未出现在本次查找结果中/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '保存配置' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: '选择“新输入框”' }));
    expect(screen.queryByText(/当前填写的名称未出现在本次查找结果中/)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '保存配置' })).toBeEnabled();

    await user.click(editButtons[1]);
    expect(screen.queryByRole('button', { name: '选择“新输入框”' })).not.toBeInTheDocument();
    await user.click(screen.getByLabelText('允许填写消息草稿'));
    expect(screen.getByLabelText(/草稿输入框名称/)).toHaveValue('');
  });

  it('does not carry a draft target across a changed executable path', async () => {
    vi.mocked(getDesktopTrustedApps).mockResolvedValue([{ ...savedApp, capabilities: ['launch', 'observe', 'observeImage', 'draft'], draftSelector: '原输入框' }]);
    const user = userEvent.setup();
    render(<DesktopControlSettings />);
    await user.click(await screen.findByRole('button', { name: '编辑' }));
    await user.clear(screen.getByLabelText('程序路径'));
    await user.type(screen.getByLabelText('程序路径'), 'C:\\Apps\\replacement.exe');

    expect(screen.getByRole('button', { name: '查找当前窗口的输入框' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '保存配置' })).toBeDisabled();
    expect(screen.getByText(/先取消草稿权限并保存新路径/)).toBeInTheDocument();
    await user.click(screen.getByLabelText('允许填写消息草稿'));
    expect(screen.getByRole('button', { name: '保存配置' })).toBeEnabled();
  });

  it('does not carry button interaction authority to another executable', async () => {
    vi.mocked(getDesktopTrustedApps).mockResolvedValue([{ ...savedApp, capabilities: ['launch', 'observe', 'observeImage', 'interact'] }]);
    const user = userEvent.setup();
    render(<DesktopControlSettings />);
    await user.click(await screen.findByRole('button', { name: '编辑' }));
    expect(screen.getByLabelText('操作控件')).toBeChecked();
    await user.clear(screen.getByLabelText('程序路径'));
    await user.type(screen.getByLabelText('程序路径'), 'C:\\Apps\\replacement.exe');

    expect(screen.getByRole('button', { name: '保存配置' })).toBeDisabled();
    expect(screen.getByText(/先取消操作控件权限并保存新路径/)).toBeInTheDocument();
    await user.click(screen.getByLabelText('操作控件'));
    expect(screen.getByRole('button', { name: '保存配置' })).toBeEnabled();
  });

  it('does not carry generic text filling authority to another executable', async () => {
    vi.mocked(getDesktopTrustedApps).mockResolvedValue([{ ...savedApp, capabilities: ['launch', 'observe', 'observeImage', 'fill'] }]);
    const user = userEvent.setup();
    render(<DesktopControlSettings />);
    await user.click(await screen.findByRole('button', { name: '编辑' }));
    expect(screen.getByLabelText('允许填写普通文本字段')).toBeChecked();
    await user.clear(screen.getByLabelText('程序路径'));
    await user.type(screen.getByLabelText('程序路径'), 'C:\\Apps\\replacement.exe');

    expect(screen.getByRole('button', { name: '保存配置' })).toBeDisabled();
    expect(screen.getByText(/先取消普通文本字段填写权限/)).toBeInTheDocument();
    await user.click(screen.getByLabelText('允许填写普通文本字段'));
    expect(screen.getByRole('button', { name: '保存配置' })).toBeEnabled();
  });
});
