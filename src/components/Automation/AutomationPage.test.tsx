import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { createAutomation, getAutomationRuns, getAutomations, runAutomationNow, setAutomationEnabled } from '$lib/commands/automation';
import { getWorkspaces } from '$lib/commands/workspace';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';
import { AutomationPage } from './AutomationPage';

vi.mock('$lib/commands/automation', () => ({
  createAutomation: vi.fn(),
  deleteAutomation: vi.fn(),
  getAutomationRuns: vi.fn(),
  getAutomations: vi.fn(),
  runAutomationNow: vi.fn(),
  setAutomationEnabled: vi.fn(),
}));

vi.mock('$lib/commands/workspace', () => ({
  getWorkspaces: vi.fn(),
}));

const getAutomationsMock = vi.mocked(getAutomations);
const getAutomationRunsMock = vi.mocked(getAutomationRuns);
const getWorkspacesMock = vi.mocked(getWorkspaces);
const createAutomationMock = vi.mocked(createAutomation);
const runAutomationNowMock = vi.mocked(runAutomationNow);
const setAutomationEnabledMock = vi.mocked(setAutomationEnabled);

const agentAutomation = {
  id: 'agent-once',
  title: '整理待办',
  prompt: '整理待办',
  triggerKind: 'once',
  triggerValue: '2026-09-19T09:00:00+08:00',
  enabled: false,
  permissionSummary: '每次运行前询问',
  executorKind: 'agent',
  workspaceId: 'project-a',
  scriptArgs: [],
  timeoutSeconds: 300,
  lastRunAt: 1_789_779_600,
};

describe('AutomationPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getAutomationsMock.mockResolvedValue([]);
    getAutomationRunsMock.mockResolvedValue([]);
    getWorkspacesMock.mockResolvedValue([
      { id: 'personal', name: 'AngelBot 日常', kind: 'personal', createdAt: 1, updatedAt: 1, activeSessionId: 'personal-main' },
      { id: 'project-a', name: '项目 A', kind: 'project', rootPath: 'C:/project-a', createdAt: 1, updatedAt: 1, activeSessionId: 'project-a-main' },
    ]);
    useNavigationStore.setState({ currentPage: 'tasks' });
    useWorkspacesStore.setState({
      workspaces: [],
      activeWorkspaceId: 'personal',
      loading: false,
      lastError: null,
      loadWorkspaces: vi.fn().mockResolvedValue(undefined),
      openWorkspace: vi.fn().mockResolvedValue(undefined),
      createProject: vi.fn().mockResolvedValue(undefined),
    });
    createAutomationMock.mockResolvedValue({
      id: 'automation-a', title: '整理笔记', prompt: '整理笔记', triggerKind: 'schedule', triggerValue: '每天 21:00', enabled: true,
      permissionSummary: '每次运行前询问', executorKind: 'agent', workspaceId: 'personal', scriptArgs: [], timeoutSeconds: 300,
    });
  });

  it('binds an Agent automation to the selected workspace before creation', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);

    await user.type(screen.getByPlaceholderText('每天晚上九点提醒我整理当天笔记'), '每天 21:00 提醒我整理笔记');
    await user.click(screen.getByRole('button', { name: '解析' }));

    await screen.findByText('归属工作区');
    const comboboxes = screen.getAllByRole('combobox');
    const workspace = comboboxes[comboboxes.length - 1];
    expect(workspace).toHaveValue('personal');
    await user.selectOptions(workspace, 'project-a');
    await user.click(screen.getByRole('button', { name: '创建任务' }));

    await waitFor(() => expect(createAutomationMock).toHaveBeenCalledWith(expect.objectContaining({
      executorKind: 'agent',
      workspaceId: 'project-a',
      triggerKind: 'schedule',
      triggerValue: '每天 21:00',
    })));
  });

  it.each([
    ['每周一 09:30 提醒我写周报', '重复自动化目前只支持每天'],
    ['工作日 08:30 提醒我站会', '重复自动化目前只支持每天'],
    ['明天 15:00 提醒我打电话', '一次性提醒请在对话中创建'],
  ])('does not create a daily automation from unsupported timing: %s', async (description, guidance) => {
    const user = userEvent.setup();
    render(<AutomationPage />);

    await user.type(screen.getByPlaceholderText('每天晚上九点提醒我整理当天笔记'), description);
    await user.click(screen.getByRole('button', { name: '解析' }));
    const createButton = screen.queryByRole('button', { name: '创建任务' });
    if (createButton) await user.click(createButton);

    expect(createAutomationMock).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toHaveTextContent(guidance);
  });

  it.each(['每周一 09:30', '工作日 08:30', '明天 15:00'])('does not submit unsupported timing from the script form: %s', async (timing) => {
    const user = userEvent.setup();
    render(<AutomationPage />);

    await user.type(screen.getByLabelText('任务名称'), '生成日报');
    await user.clear(screen.getByRole('textbox', { name: /^触发时间/ }));
    await user.type(screen.getByRole('textbox', { name: /^触发时间/ }), timing);
    await user.type(screen.getByLabelText('脚本路径'), 'scripts/report.py');
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    const createButton = screen.queryByRole('button', { name: '创建任务' });
    if (createButton) await user.click(createButton);

    expect(createAutomationMock).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toHaveTextContent('重复自动化目前只支持每天');
  });

  it('previews and creates a script only with the canonical daily schedule', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);

    await user.type(screen.getByLabelText('任务名称'), '生成日报');
    await user.clear(screen.getByRole('textbox', { name: /^触发时间/ }));
    await user.type(screen.getByRole('textbox', { name: /^触发时间/ }), '9:30');
    await user.type(screen.getByLabelText('脚本路径'), 'scripts/report.py');
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    expect(screen.getByRole('status')).toHaveTextContent('生成日报 · 每天 09:30');
    await user.click(screen.getByRole('button', { name: '创建任务' }));

    await waitFor(() => expect(createAutomationMock).toHaveBeenCalledWith(expect.objectContaining({
      executorKind: 'script', triggerKind: 'schedule', triggerValue: '每天 09:30',
    })));
  });

  it('invalidates the parsed daily draft when the description changes to a one-time reminder', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);
    const description = screen.getByPlaceholderText('每天晚上九点提醒我整理当天笔记');
    await user.type(description, '每天 21:00 提醒我整理笔记');
    await user.click(screen.getByRole('button', { name: '解析' }));
    expect(screen.getByRole('button', { name: '创建任务' })).toBeInTheDocument();

    await user.clear(description);
    await user.type(description, '明天 15:00 提醒我打电话');
    const createButton = screen.queryByRole('button', { name: '创建任务' });
    if (createButton) await user.click(createButton);

    expect(createAutomationMock).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: '创建任务' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '解析' }));
    expect(screen.getByRole('alert')).toHaveTextContent('一次性提醒请在对话中创建');
  });

  it('presents a completed one-time reminder as a local notification, not a script', async () => {
    getAutomationsMock.mockResolvedValueOnce([{
      id: 'reminder-a',
      title: '查看会议资料',
      prompt: '记得查看明天会议的材料。',
      triggerKind: 'once',
      triggerValue: '2026-09-19T09:00:00+08:00',
      enabled: false,
      permissionSummary: '到点发送本地提醒',
      executorKind: 'notification',
      workspaceId: 'personal',
      scriptArgs: [],
      timeoutSeconds: 300,
      lastRunAt: 1_789_779_600,
    }]);

    render(<AutomationPage />);

    expect(await screen.findByText('本地通知 · AngelBot 日常')).toBeInTheDocument();
    expect(screen.getByText('已提醒')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '已完成' })).toBeDisabled();
    expect(screen.queryByText('未配置脚本')).not.toBeInTheDocument();
  });

  it('distinguishes a cancelled reminder from a paused recurring task', async () => {
    getAutomationsMock.mockResolvedValueOnce([{
      id: 'reminder-cancelled',
      title: '已经取消的提醒',
      prompt: '不再触发',
      triggerKind: 'once',
      triggerValue: '2099-09-19T09:00:00+08:00',
      enabled: false,
      permissionSummary: '到点发送本地提醒',
      executorKind: 'notification',
      workspaceId: 'personal',
      scriptArgs: [],
      timeoutSeconds: 300,
    }]);

    const { container } = render(<AutomationPage />);

    await screen.findByText('已经取消的提醒');
    expect(container.querySelector('.automation-status')).toHaveTextContent('已取消');
    expect(screen.getByRole('button', { name: '恢复' })).toBeEnabled();
  });

  it('requires enabling a paused Agent definition before immediate execution', async () => {
    const user = userEvent.setup();
    const paused = {
      ...agentAutomation, id: 'daily-agent', triggerKind: 'schedule',
      triggerValue: '每天 09:00', lastRunAt: undefined,
    };
    getAutomationsMock.mockResolvedValueOnce([paused]).mockResolvedValue([{ ...paused, enabled: true }]);
    setAutomationEnabledMock.mockResolvedValue(undefined);
    runAutomationNowMock.mockResolvedValue(undefined);
    render(<AutomationPage />);

    const runButton = await screen.findByRole('button', { name: '先启用再执行' });
    expect(runButton).toBeDisabled();
    await user.click(runButton);
    expect(runAutomationNowMock).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: '启用' }));
    await waitFor(() => expect(setAutomationEnabledMock).toHaveBeenCalledWith('daily-agent', true));
    const enabledRunButton = await screen.findByRole('button', { name: '立即执行' });
    expect(enabledRunButton).toBeEnabled();
    await user.click(enabledRunButton);
    await waitFor(() => expect(runAutomationNowMock).toHaveBeenCalledWith('daily-agent'));
  });

  it.each([
    ['script', '立即运行'],
    ['notification', '立即提醒'],
  ])('preserves manual execution for a disabled %s definition', async (executorKind, buttonLabel) => {
    const user = userEvent.setup();
    const item = { ...agentAutomation, id: `paused-${executorKind}`, executorKind };
    getAutomationsMock.mockResolvedValue([item]);
    runAutomationNowMock.mockResolvedValue(undefined);
    render(<AutomationPage />);

    const runButton = await screen.findByRole('button', { name: buttonLabel });
    expect(runButton).toBeEnabled();
    await user.click(runButton);
    await waitFor(() => expect(runAutomationNowMock).toHaveBeenCalledWith(item.id));
  });

  it.each([
    ['queued', '排队中', '等待主 Agent 处理'],
    ['running', '执行中', '主 Agent 正在处理'],
    ['awaiting_confirmation', '等待确认', '等待你的确认'],
    ['needs_attention', '需要处理', '请在归属工作区继续处理'],
    ['completed', '已完成', '已完成'],
  ])('projects a one-time Agent schedule from its latest %s run', async (status, label, detail) => {
    getAutomationsMock.mockResolvedValueOnce([agentAutomation]);
    getAutomationRunsMock.mockResolvedValueOnce([{
      id: `run-${status}`,
      status,
      summary: `当前状态：${label}`,
      output: '',
      startedAt: 1_789_779_600,
    }]);

    render(<AutomationPage />);

    expect((await screen.findAllByText(label)).length).toBeGreaterThan(0);
    expect(screen.getAllByText(detail).length).toBeGreaterThan(0);
    expect(screen.queryByText('已暂停')).not.toBeInTheDocument();
  });

  it('opens the owning main conversation when an Agent automation awaits confirmation', async () => {
    const user = userEvent.setup();
    const openWorkspace = vi.fn().mockResolvedValue(undefined);
    useWorkspacesStore.setState({ openWorkspace });
    getAutomationsMock.mockResolvedValueOnce([agentAutomation]);
    getAutomationRunsMock.mockResolvedValueOnce([{
      id: 'run-confirmation',
      status: 'awaiting_confirmation',
      summary: '主 Agent 需要确认',
      output: '',
      startedAt: 1_789_779_600,
    }]);

    render(<AutomationPage />);

    await user.click(await screen.findByRole('button', { name: '前往 项目 A 确认' }));
    await waitFor(() => expect(openWorkspace).toHaveBeenCalledWith('project-a'));
    expect(useNavigationStore.getState().currentPage).toBe('chat');
  });
});
