import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { createAutomation, createMaaAutomation, deleteAutomation, getAutomationRuns, getAutomationDashboard, runAutomationNow, setAutomationEnabled, stopAutomationRun, type Automation, type AutomationRun } from '$lib/commands/automation';
import { getDesktopTrustedApps, type TrustedDesktopApp } from '$lib/commands/desktop';
import { AUTOMATIONS_UPDATED_EVENT } from '$lib/automation-events';
import { getWorkspaces } from '$lib/commands/workspace';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';
import { AutomationPage } from './AutomationPage';

vi.mock('$lib/commands/automation', () => ({
  createAutomation: vi.fn(),
  createMaaAutomation: vi.fn(),
  deleteAutomation: vi.fn(),
  getAutomationRuns: vi.fn(),
  getAutomationDashboard: vi.fn(),
  runAutomationNow: vi.fn(),
  setAutomationEnabled: vi.fn(),
  stopAutomationRun: vi.fn(),
}));

vi.mock('$lib/commands/workspace', () => ({
  getWorkspaces: vi.fn(),
}));
vi.mock('$lib/commands/desktop', () => ({ getDesktopTrustedApps: vi.fn() }));

// Scenario builders only: production receives a joined dashboard, never N histories.
// Priority itself is covered by the Rust read-projection tests.
const getAutomationsMock = vi.fn<() => Promise<Automation[]>>();
const getAutomationRunsMock = vi.fn<(id: string) => Promise<AutomationRun[]>>();
const getDashboardMock = vi.mocked(getAutomationDashboard);
const getHistoryMock = vi.mocked(getAutomationRuns);
const getWorkspacesMock = vi.mocked(getWorkspaces);
const createAutomationMock = vi.mocked(createAutomation);
const createMaaAutomationMock = vi.mocked(createMaaAutomation);
const getDesktopTrustedAppsMock = vi.mocked(getDesktopTrustedApps);
const runAutomationNowMock = vi.mocked(runAutomationNow);
const setAutomationEnabledMock = vi.mocked(setAutomationEnabled);
const stopAutomationRunMock = vi.mocked(stopAutomationRun);
const deleteAutomationMock = vi.mocked(deleteAutomation);

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

const scriptAutomation = {
  ...agentAutomation, id: 'daily-script', title: '现有脚本', executorKind: 'script',
  triggerKind: 'schedule', triggerValue: '每天 18:00', enabled: true, scriptPath: 'scripts/daily.ps1',
};
const maaAutomation = { ...scriptAutomation, id: 'daily-maa', title: 'MAA 日常队列', executorKind: 'maa', scriptPath: undefined, timeoutSeconds: 3600 };
const trustedMaaApp: TrustedDesktopApp = {
  id: 'maa-app', displayName: '测试 MAA', executablePath: 'C:/Game Tools/MAA.exe',
  capabilities: ['launch'], draftSelector: null, enabled: true, createdAt: 1, updatedAt: 1,
};

describe('AutomationPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getAutomationsMock.mockResolvedValue([]);
    getAutomationRunsMock.mockResolvedValue([]);
    getHistoryMock.mockResolvedValue([]);
    getDashboardMock.mockImplementation(async () => ({ items: await Promise.all((await getAutomationsMock()).map(async item => {
      const runs = await getAutomationRunsMock(item.id);
      const eligible = runs.filter(run => run.status !== 'skipped' || item.executorKind === 'notification');
      const latestRun = eligible.find(run => run.status === 'running')
        ?? eligible.find(run => ['queued', 'awaiting_confirmation'].includes(run.status)) ?? eligible[0] ?? null;
      return { ...item, latestRun, lastSkippedAt: runs[0]?.status === 'skipped' ? runs[0].startedAt : null };
    })) }));
    getDesktopTrustedAppsMock.mockResolvedValue([]);
    createMaaAutomationMock.mockResolvedValue(maaAutomation);
    runAutomationNowMock.mockResolvedValue(undefined);
    setAutomationEnabledMock.mockResolvedValue(undefined);
    stopAutomationRunMock.mockResolvedValue(undefined);
    deleteAutomationMock.mockResolvedValue(undefined);
    getWorkspacesMock.mockResolvedValue([
      { id: 'personal', name: 'AngelBot 日常', kind: 'personal', createdAt: 1, updatedAt: 1, activeSessionId: 'personal-main' },
      { id: 'project-a', name: '项目 A', kind: 'project', rootPath: 'C:/project-a', createdAt: 1, updatedAt: 1, activeSessionId: 'project-a-main' },
    ]);
    useNavigationStore.setState({ currentPage: 'tasks', automationTarget: null });
    useWorkspacesStore.setState({
      workspaces: [],
      activeWorkspaceId: 'personal',
      loading: false,
      lastError: null,
      loadWorkspaces: vi.fn().mockResolvedValue(undefined),
      openWorkspace: vi.fn().mockResolvedValue(true),
      createProject: vi.fn().mockResolvedValue(true),
    });
    createAutomationMock.mockResolvedValue({
      id: 'automation-a', title: '整理笔记', prompt: '整理笔记', triggerKind: 'schedule', triggerValue: '每天 21:00', enabled: true,
      permissionSummary: '每次运行前询问', executorKind: 'agent', workspaceId: 'personal', scriptArgs: [], timeoutSeconds: 300,
    });
  });

  it('uses one dashboard read and lazily loads history with explicit failure retry', async () => {
    getDashboardMock.mockResolvedValue({ items: [{ ...scriptAutomation, latestRun: {
      id: 'head', status: 'completed', summary: '权威回执', startedAt: 1, exitCode: 0,
    }, lastSkippedAt: 2 }] });
    getHistoryMock.mockRejectedValueOnce(new Error('history unavailable')).mockResolvedValue([{
      id: 'old', status: 'failed', summary: '历史失败', startedAt: 1, output: '历史输出', exitCode: 1,
    }]);
    render(<AutomationPage />);
    await screen.findByText('权威回执');
    expect(getDashboardMock).toHaveBeenCalledTimes(1);
    expect(getHistoryMock).not.toHaveBeenCalled();
    const details = screen.getByText('运行记录与输出').closest('details')!;
    details.open = true;
    fireEvent(details, new Event('toggle'));
    await screen.findByText('无法读取运行记录，当前任务状态未改变。');
    expect(screen.getByText('权威回执')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '重试读取记录' }));
    await screen.findByText('历史输出');
    expect(screen.getByText('权威回执')).toBeInTheDocument();
    expect(screen.getByText('脚本已结束')).toBeInTheDocument();
    expect(getHistoryMock).toHaveBeenCalledTimes(2);
    expect(runAutomationNowMock).not.toHaveBeenCalled();
  });

  it('focuses an exact owner-bound target once and never runs it through navigation', async () => {
    const payload = { items: [{ ...scriptAutomation, latestRun: null, lastSkippedAt: null }] };
    getDashboardMock.mockResolvedValue(payload);
    useNavigationStore.getState().openAutomation({ automationId: scriptAutomation.id, workspaceId: 'project-a' });
    render(<AutomationPage />);
    const card = await screen.findByRole('article', { name: '现有脚本' });
    await waitFor(() => expect(card).toHaveFocus());
    const refresh = screen.getByRole('button', { name: '刷新' });
    refresh.focus();
    fireEvent.click(refresh);
    await waitFor(() => expect(getDashboardMock).toHaveBeenCalledTimes(2));
    expect(refresh).toHaveFocus();
    expect(getHistoryMock).not.toHaveBeenCalled();
    act(() => useNavigationStore.getState().openAutomation({ automationId: scriptAutomation.id, workspaceId: 'personal' }));
    expect(await screen.findByText(/无法定位该事项/)).toBeInTheDocument();
    expect(card).not.toHaveClass('automation-card-target');
    expect(runAutomationNowMock).not.toHaveBeenCalled();
    expect(stopAutomationRunMock).not.toHaveBeenCalled();
  });

  it('ignores a closed history request and keeps an owner-pinned unknown head authoritative', async () => {
    let finishHistory!: (runs: AutomationRun[]) => void;
    getHistoryMock.mockReturnValueOnce(new Promise(resolve => { finishHistory = resolve; })).mockResolvedValueOnce([
      { id: 'current-history', status: 'failed', summary: '本次读取的记录', output: '', startedAt: 3 },
    ]);
    getDashboardMock.mockResolvedValue({ items: [{ ...scriptAutomation, enabled: false, latestRun: {
      id: 'pinned', status: 'attention_required', summary: '归属未释放，结果未知', startedAt: 1,
    }, lastSkippedAt: null }] });
    const { unmount } = render(<AutomationPage />);
    const details = (await screen.findByText('运行记录与输出')).closest('details')!;
    details.open = true;
    fireEvent(details, new Event('toggle'));
    await waitFor(() => expect(getHistoryMock).toHaveBeenCalledTimes(1));
    details.open = false;
    fireEvent(details, new Event('toggle'));
    await act(async () => {});
    details.open = true;
    fireEvent(details, new Event('toggle'));
    await screen.findByText('本次读取的记录');
    await act(async () => { finishHistory([{ id: 'unrelated-live', status: 'running', summary: '旧输出', output: '迟到的输出', startedAt: 2, stopControl: 'available' }]); });
    expect(screen.queryByText('迟到的输出')).not.toBeInTheDocument();
    expect(screen.getByText('本次读取的记录')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '停止本次执行' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '启用' })).toBeDisabled();
    unmount();
  });

  it('binds an Agent automation to the selected workspace before creation', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);

    await user.type(screen.getByPlaceholderText('每天晚上九点提醒我整理当天笔记'), '每天 21:00 提醒我整理笔记');
    await user.click(screen.getByRole('button', { name: '解析' }));

    await screen.findByText('归属工作区');
    const workspace = screen.getByRole('combobox', { name: /^归属工作区/ });
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
      executorKind: 'script', triggerKind: 'schedule', triggerValue: '每天 09:30', scriptArgs: [],
    })));
  });

  it('preserves each argument line including spaces and literal quotes', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);

    await user.type(screen.getByLabelText('任务名称'), '运行现有流程');
    await user.type(screen.getByLabelText('脚本路径'), 'scripts/daily.ps1');
    fireEvent.change(screen.getByRole('textbox', { name: /^参数/ }), {
      target: { value: '--profile\r\nC:\\Game Tools\\daily.json\r\n\r\n  spaced argument  \r\n"literal quotes"' },
    });
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    await user.click(screen.getByRole('button', { name: '创建任务' }));

    await waitFor(() => expect(createAutomationMock).toHaveBeenCalledWith(expect.objectContaining({
      scriptArgs: ['--profile', 'C:\\Game Tools\\daily.json', '  spaced argument  ', '"literal quotes"'],
    })));
  });

  it('creates a parsed script without promising confirmation before each run', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);

    await user.type(screen.getByPlaceholderText('每天晚上九点提醒我整理当天笔记'), '每天 21:00 运行脚本 scripts/daily.ps1 执行现有流程');
    await user.click(screen.getByRole('button', { name: '解析' }));
    expect(screen.getByText(/不会在每次运行前询问/)).toBeInTheDocument();
    expect(screen.getByText(/脚本结束或超时会回收其自有进程树/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '创建任务' }));

    await waitFor(() => expect(createAutomationMock).toHaveBeenCalledWith(expect.objectContaining({
      executorKind: 'script', scriptPath: 'scripts/daily.ps1', scriptArgs: [],
      permissionSummary: '仅执行已配置的本地脚本；输出和退出码会保存在运行记录中。',
    })));
  });

  it.each(['manual', 'parsed'])('shows native batch validation errors in the %s creation form', async entry => {
    const user = userEvent.setup();
    const reason = '批处理脚本路径和参数不能包含 shell 元字符、引号或控制字符';
    createAutomationMock.mockRejectedValueOnce(reason);
    render(<AutomationPage />);

    if (entry === 'manual') {
      await user.type(screen.getByLabelText('任务名称'), '现有脚本');
      await user.type(screen.getByLabelText('脚本路径'), 'scripts/daily.cmd');
      await user.click(screen.getByRole('button', { name: '预览规则' }));
    } else {
      await user.type(screen.getByPlaceholderText('每天晚上九点提醒我整理当天笔记'), '每天 21:00 运行脚本 scripts/daily.cmd 执行现有流程');
      await user.click(screen.getByRole('button', { name: '解析' }));
    }
    expect(screen.getByText(/脚本结束或超时会回收其自有进程树/)).toBeInTheDocument();
    expect(screen.getByText(/批处理（.bat\/.cmd）不接受引号或 shell 元字符/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '创建任务' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(reason);
    expect(screen.getByRole('button', { name: '创建任务' })).toBeEnabled();
  });

  it('refreshes completed script results on desktop updates and removes the listener on unmount', async () => {
    getAutomationsMock.mockResolvedValue([{
      ...agentAutomation, id: 'daily-script', title: '现有脚本', executorKind: 'script',
      triggerKind: 'schedule', triggerValue: '每天 09:00', enabled: true, scriptPath: 'scripts/daily.ps1',
    }]);
    getAutomationRunsMock.mockResolvedValueOnce([]).mockResolvedValue([{
      id: 'script-run', status: 'completed', summary: '脚本退出码为 0', exitCode: 0,
      output: 'process exited', startedAt: 1_789_779_600,
    }]);
    const { container, unmount } = render(<AutomationPage />);

    await screen.findByText('尚无运行记录');
    fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT));

    expect(await screen.findByText('脚本退出码为 0')).toBeInTheDocument();
    expect(getDashboardMock).toHaveBeenCalledTimes(2);
    expect(getHistoryMock).not.toHaveBeenCalled();
    expect(container.querySelector('.automation-status')).toHaveTextContent('脚本已结束');
    expect(screen.getByText(/每日任务仅在 AngelBot 运行时检查执行/)).toBeInTheDocument();
    expect(screen.getByText(/运行结果依据脚本退出码，不表示软件打开或日常任务已完成/)).toBeInTheDocument();
    expect(screen.getByText(/GUI 辅助工具需要支持按需启动与明确的任务控制接口/)).toBeInTheDocument();
    expect(screen.getByText(/本页只执行等待自身任务结束的脚本/)).toBeInTheDocument();
    expect(screen.queryByText(/GUI 辅助工具优先使用已有定时功能/)).not.toBeInTheDocument();
    expect(screen.getByText(/暂停仅影响后续定时执行，不会停止已启动的脚本/)).toBeInTheDocument();

    const loadCount = getAutomationsMock.mock.calls.length;
    unmount();
    fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT));
    expect(getAutomationsMock).toHaveBeenCalledTimes(loadCount);
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
    expect(screen.getByText('提醒已触发')).toBeInTheDocument();
    expect(screen.getByText(/不代表 Windows 通知已送达/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '已完成' })).toBeDisabled();
    expect(screen.queryByText('未配置脚本')).not.toBeInTheDocument();
  });

  it.each([undefined, 1_789_779_600])('preserves notification failure with lastRunAt %s', async lastRunAt => {
    getAutomationsMock.mockResolvedValue([{
      ...agentAutomation, id: 'reminder-failed', executorKind: 'notification', lastRunAt,
    }]);
    getAutomationRunsMock.mockResolvedValue([{
      id: 'failed-run', status: 'needs_attention',
      summary: '无法请求系统通知', output: '', startedAt: 10,
    }]);
    const { container } = render(<AutomationPage />);
    await screen.findByText('无法请求系统通知');
    expect(container.querySelector('.automation-status')).toHaveTextContent('需要处理');
    expect(screen.queryByText('提醒已触发')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '立即提醒' })).toBeEnabled();
    expect(screen.getByRole('button', { name: '需检查通知' })).toBeDisabled();
    expect(screen.getByText(/不会自动补发/)).toBeInTheDocument();
  });

  it('does not present a muted notification as completed or delivered', async () => {
    getAutomationsMock.mockResolvedValue([{
      ...agentAutomation, id: 'muted', executorKind: 'notification', lastRunAt: 1_789_779_600,
    }]);
    getAutomationRunsMock.mockResolvedValue([{
      id: 'muted-run', status: 'skipped',
      summary: '通知已关闭，本次提醒未发送；不会自动补发', output: '', startedAt: 10,
    }]);
    const { container } = render(<AutomationPage />);
    await screen.findAllByText('通知已关闭，本次提醒未发送；不会自动补发');
    expect(container.querySelector('.automation-status')).toHaveTextContent('本次未发送');
    expect(screen.queryByText('提醒已触发')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '已完成' })).not.toBeInTheDocument();
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
    ['once', 'queued', '排队中', '等待主 Agent 处理', false],
    ['once', 'running', '执行中', '主 Agent 正在处理', false],
    ['once', 'awaiting_confirmation', '等待确认', '等待你的确认', false],
    ['once', 'needs_attention', '需要处理', '请在归属工作区继续处理', false],
    ['once', 'completed', '已完成', '已完成', false],
    ['schedule', 'queued', '排队中', '', true],
    ['schedule', 'running', '执行中', '', true],
    ['schedule', 'awaiting_confirmation', '等待确认', '', true],
    ['schedule', 'needs_attention', '需要处理', '', true],
    ['schedule', 'failed', '执行失败', '', true],
    ['schedule', 'interrupted', '已中断', '', true],
    ['schedule', 'completed', '已完成', '', true],
    ['schedule', 'completed', '已完成', '', false],
  ] as const)('projects an Agent %s definition separately from its latest %s run', async (triggerKind, status, label, detail, enabled) => {
    getAutomationsMock.mockResolvedValueOnce([{
      ...agentAutomation, triggerKind, enabled, nextRunAt: 1_789_779_600,
    }]);
    getAutomationRunsMock.mockResolvedValueOnce([{
      id: `run-${status}`,
      status,
      summary: `当前状态：${label}`,
      output: '',
      startedAt: 1_789_779_600,
      stopControl: 'available',
    }]);

    render(<AutomationPage />);

    expect((await screen.findAllByText(label)).length).toBeGreaterThan(0);
    expect(screen.queryByRole('button', { name: '停止本次执行' })).not.toBeInTheDocument();
    if (triggerKind === 'once') {
      expect(screen.getAllByText(detail).length).toBeGreaterThan(0);
      expect(screen.queryByText('已暂停')).not.toBeInTheDocument();
    } else {
      expect(screen.getByText('下次执行').nextElementSibling).toHaveTextContent(enabled ? '2026' : '已暂停');
      expect(screen.queryByText('本次状态')).not.toBeInTheDocument();
      expect(screen.getByRole('button', { name: enabled ? '暂停' : '启用' })).toBeEnabled();
    }
    if (['queued', 'running', 'awaiting_confirmation'].includes(status)) {
      const runButton = screen.getByRole('button', { name: label });
      expect(runButton).toBeDisabled();
      fireEvent.click(runButton);
      expect(runAutomationNowMock).not.toHaveBeenCalled();
    }
  });

  it.each([
    ['once', 'awaiting_confirmation', '确认'],
    ['schedule', 'awaiting_confirmation', '确认'],
    ['schedule', 'running', '查看进度'],
    ['schedule', 'needs_attention', '继续处理'],
    ['schedule', 'failed', '继续处理'],
    ['schedule', 'interrupted', '继续处理'],
    ['schedule', 'completed', '查看结果'],
  ])('opens the owning main conversation for a %s Agent %s receipt without rerunning it', async (triggerKind, status, action) => {
    const user = userEvent.setup();
    const openWorkspace = vi.fn().mockResolvedValue(true);
    useWorkspacesStore.setState({ openWorkspace });
    getAutomationsMock.mockResolvedValueOnce([{ ...agentAutomation, triggerKind, enabled: triggerKind === 'schedule' }]);
    getAutomationRunsMock.mockResolvedValueOnce([{
      id: `run-${status}`,
      status,
      summary: `主 Agent：${status}`,
      output: '',
      startedAt: 1_789_779_600,
    }]);

    render(<AutomationPage />);

    await user.click(await screen.findByRole('button', { name: `前往 项目 A ${action}` }));
    await waitFor(() => expect(openWorkspace).toHaveBeenCalledWith('project-a'));
    expect(useNavigationStore.getState().currentPage).toBe('chat');
    expect(runAutomationNowMock).not.toHaveBeenCalled();
    expect(setAutomationEnabledMock).not.toHaveBeenCalled();
    expect(createAutomationMock).not.toHaveBeenCalled();
  });

  it('keeps the running script receipt visible ahead of a newer skipped occurrence and allows pausing', async () => {
    const user = userEvent.setup();
    getAutomationsMock.mockResolvedValue([scriptAutomation]);
    getAutomationRunsMock.mockResolvedValue([
      { id: 'skipped', status: 'skipped', summary: '已有运行，跳过本次定时', output: '', startedAt: 2 },
      { id: 'active', status: 'running', summary: '脚本启动已登记', output: '', startedAt: 1 },
    ]);
    render(<AutomationPage />);

    expect(await screen.findByText('已启动，等待结果')).toBeInTheDocument();
    expect(screen.getByText('脚本启动已登记')).toBeInTheDocument();
    expect(screen.getByText(/重启后不能仅凭此回执确认脚本仍在运行/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '等待结果' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: '停止本次执行' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '删除 现有脚本' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: '暂停' }));
    await waitFor(() => expect(setAutomationEnabledMock).toHaveBeenCalledWith('daily-script', false));
    expect(runAutomationNowMock).not.toHaveBeenCalled();
    expect(deleteAutomationMock).not.toHaveBeenCalled();
  });

  it.each([scriptAutomation, maaAutomation])('requests stopping only the selected owned $executorKind run and waits for its authoritative receipt', async (item) => {
    const user = userEvent.setup();
    const skipped = { id: 'newer-skipped', status: 'skipped', summary: '跳过本次定时', output: '', startedAt: 2 };
    const running = { id: 'owned-local-run', status: 'running', summary: '本次任务正在执行', output: '', startedAt: 1 };
    getAutomationsMock.mockResolvedValue([item]);
    getAutomationRunsMock.mockResolvedValueOnce([skipped, { ...running, stopControl: 'available' }])
      .mockResolvedValue([skipped, { ...running, stopControl: 'requested' }]);
    render(<AutomationPage />);

    await user.click(await screen.findByRole('button', { name: '停止本次执行' }));

    await waitFor(() => expect(stopAutomationRunMock).toHaveBeenCalledWith(item.id, 'owned-local-run'));
    expect(await screen.findByRole('button', { name: '正在停止' })).toBeDisabled();
    expect(screen.getByText(running.summary)).toBeInTheDocument();
    expect(screen.getByText(/正在清理.*结束回执/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '暂停' })).toBeEnabled();
    expect(runAutomationNowMock).not.toHaveBeenCalled();
    expect(setAutomationEnabledMock).not.toHaveBeenCalled();
    expect(deleteAutomationMock).not.toHaveBeenCalled();
  });

  it.each(['request', 'refresh'])('does not fabricate a stopped receipt after a stop %s failure', async (phase) => {
    const user = userEvent.setup();
    const running = { id: 'owned-run', status: 'running', summary: '本次仍未取得结束回执', output: '', startedAt: 1, stopControl: 'available' as const };
    getAutomationsMock.mockResolvedValue([scriptAutomation]);
    getAutomationRunsMock.mockResolvedValueOnce([running]);
    if (phase === 'request') stopAutomationRunMock.mockRejectedValueOnce('本次运行已结束，不能停止');
    else getAutomationRunsMock.mockRejectedValue(new Error('read failed'));
    render(<AutomationPage />);

    await user.click(await screen.findByRole('button', { name: '停止本次执行' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(phase === 'request' ? '本次运行已结束，不能停止' : '无法加载自动化任务');
    expect(screen.getByText(running.summary)).toBeInTheDocument();
    expect(screen.queryByText('已停止')).not.toBeInTheDocument();
    expect(stopAutomationRunMock).toHaveBeenCalledTimes(1);
    expect(runAutomationNowMock).not.toHaveBeenCalled();
    expect(setAutomationEnabledMock).not.toHaveBeenCalled();
  });

  it.each([scriptAutomation, maaAutomation])('shows cleaned-up cancelled $executorKind execution without claiming rollback or disabling its schedule', async (item) => {
    getAutomationsMock.mockResolvedValue([item]);
    getAutomationRunsMock.mockResolvedValue([{
      id: 'stopped-run', status: 'cancelled', summary: '本次受控运行已停止', output: '', startedAt: 1,
      stopControl: 'available',
    }]);
    render(<AutomationPage />);

    expect(await screen.findByText('已停止')).toBeInTheDocument();
    expect(screen.getByText(/已发生的操作不会撤回/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '暂停' })).toBeEnabled();
    expect(screen.getByRole('button', { name: '立即运行' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: '停止本次执行' })).not.toBeInTheDocument();
    expect(stopAutomationRunMock).not.toHaveBeenCalled();
    expect(setAutomationEnabledMock).not.toHaveBeenCalled();
  });

  it.each([false, true])('keeps the newer page after a result-link workspace request (accepted: %s)', async (accepted) => {
    let finish!: (accepted: boolean) => void;
    const opening = vi.fn(() => new Promise<boolean>((resolve) => { finish = resolve; }));
    useWorkspacesStore.setState({ openWorkspace: opening });
    getDashboardMock.mockResolvedValue({ items: [{ ...agentAutomation, latestRun: {
      id: 'done', status: 'completed', summary: '已完成的记录', startedAt: 1,
    }, lastSkippedAt: null }] });
    render(<AutomationPage />);
    fireEvent.click(await screen.findByRole('button', { name: '前往 项目 A 查看结果' }));
    expect(opening).toHaveBeenCalledWith('project-a');
    if (accepted) act(() => useNavigationStore.getState().setPage('knowledge'));
    await act(async () => { finish(accepted); });
    expect(useNavigationStore.getState().currentPage).toBe(accepted ? 'knowledge' : 'tasks');
    expect(runAutomationNowMock).not.toHaveBeenCalled();
  });

  it('explains unknown script results without automatically retrying a paused definition', async () => {
    getAutomationsMock.mockResolvedValue([{ ...scriptAutomation, enabled: false }]);
    getAutomationRunsMock.mockResolvedValue([
      { id: 'skipped', status: 'skipped', summary: '已有运行，跳过此次调度', output: '', startedAt: 2 },
      { id: 'unknown', status: 'attention_required', summary: '上次启动后未取得可靠结束结果', output: '', startedAt: 1, stopControl: 'available' },
    ]);
    render(<AutomationPage />);

    expect(await screen.findByText('需要处理')).toBeInTheDocument();
    expect(screen.getByText(/这次运行结果未知，自动调度已暂停，不会自动重试/)).toBeInTheDocument();
    expect(screen.getByText(/得到明确结果后才能重新启用定时/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '核对后重新运行' })).toBeEnabled();
    expect(screen.getByRole('button', { name: '启用' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: '停止本次执行' })).not.toBeInTheDocument();
    expect(runAutomationNowMock).not.toHaveBeenCalled();
    expect(setAutomationEnabledMock).not.toHaveBeenCalled();
  });

  it('blocks duplicate run and delete while its own request is pending but still allows owned stop and future pause', async () => {
    const user = userEvent.setup();
    let finishRun!: () => void;
    runAutomationNowMock.mockReturnValueOnce(new Promise<void>(resolve => { finishRun = resolve; }));
    getAutomationsMock.mockResolvedValue([scriptAutomation]);
    render(<AutomationPage />);

    await user.click(await screen.findByRole('button', { name: '立即运行' }));
    const pendingButton = screen.getByRole('button', { name: '处理中…' });
    expect(pendingButton).toBeDisabled();
    expect(screen.getByRole('button', { name: '删除 现有脚本' })).toBeDisabled();
    await user.click(pendingButton);
    const running = { id: 'pending-owned-run', status: 'running', summary: '仍在执行', output: '', startedAt: 1 };
    getAutomationRunsMock.mockResolvedValue([{ ...running, stopControl: 'available' }]);
    fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT));
    const stopButton = await screen.findByRole('button', { name: '停止本次执行' });
    getAutomationRunsMock.mockResolvedValue([{ ...running, stopControl: 'requested' }]);
    await user.click(stopButton);
    expect(stopAutomationRunMock).toHaveBeenCalledWith('daily-script', 'pending-owned-run');
    expect(await screen.findByRole('button', { name: '正在停止' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: '暂停' }));
    await waitFor(() => expect(setAutomationEnabledMock).toHaveBeenCalledWith('daily-script', false));
    expect(runAutomationNowMock).toHaveBeenCalledTimes(1);
    getAutomationRunsMock.mockResolvedValue([]);
    await act(async () => { finishRun(); });
    await waitFor(() => expect(screen.getByRole('button', { name: '立即运行' })).toBeEnabled());
  });

  it('surfaces a raw backend busy error and releases only its pending request', async () => {
    const user = userEvent.setup();
    getAutomationsMock.mockResolvedValue([scriptAutomation]);
    runAutomationNowMock.mockRejectedValueOnce('此脚本已有运行中回执，请等待结果');
    render(<AutomationPage />);

    await user.click(await screen.findByRole('button', { name: '立即运行' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('此脚本已有运行中回执，请等待结果');
    expect(screen.getByRole('button', { name: '立即运行' })).toBeEnabled();
  });

  it('does not let an older refresh replace a newer completed receipt', async () => {
    let finishOldLoad!: (items: typeof scriptAutomation[]) => void;
    getAutomationsMock.mockReturnValueOnce(new Promise(resolve => { finishOldLoad = resolve; }))
      .mockResolvedValue([scriptAutomation]);
    getAutomationRunsMock.mockResolvedValue([{
      id: 'done', status: 'completed', summary: '脚本已退出，退出码 0', exitCode: 0, output: '', startedAt: 2,
    }]);
    render(<AutomationPage />);

    fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT));
    expect(await screen.findByText('脚本已退出，退出码 0')).toBeInTheDocument();
    await act(async () => { finishOldLoad([{ ...scriptAutomation, title: '旧版本标题' }]); });
    expect(screen.getByText('现有脚本')).toBeInTheDocument();
    expect(screen.queryByText('旧版本标题')).not.toBeInTheDocument();
  });

  it('creates the current MAA queue only from a launch-enabled trusted application after explicit confirmation', async () => {
    const user = userEvent.setup();
    getDesktopTrustedAppsMock.mockResolvedValue([
      trustedMaaApp,
      { ...trustedMaaApp, id: 'disabled', displayName: '已禁用 MAA', enabled: false },
      { ...trustedMaaApp, id: 'observe-only', displayName: '仅观察 MAA', capabilities: ['observe'] },
      { ...trustedMaaApp, id: 'other-app', displayName: '其他辅助工具', executablePath: 'C:/Game Tools/Helper.exe' },
    ]);
    render(<AutomationPage />);
    await user.selectOptions(screen.getByRole('combobox', { name: '任务类型' }), 'maa');
    expect(await screen.findByRole('option', { name: '测试 MAA' })).toBeInTheDocument();
    expect(screen.queryByRole('option', { name: '已禁用 MAA' })).not.toBeInTheDocument();
    expect(screen.queryByRole('option', { name: '仅观察 MAA' })).not.toBeInTheDocument();
    expect(screen.queryByRole('option', { name: '其他辅助工具' })).not.toBeInTheDocument();
    await user.type(screen.getByLabelText('任务名称'), 'MAA 日常队列');
    await user.selectOptions(screen.getByRole('combobox', { name: /^归属工作区/ }), 'project-a');
    expect(screen.getByRole('checkbox', { name: /^按 MAA 当前配置启动模拟器/ })).not.toBeChecked();
    expect(screen.getByLabelText('超时（秒）')).toHaveValue('3600');
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    const createButton = screen.getByRole('button', { name: '确认并创建 MAA 任务' });
    expect(createButton).toBeDisabled();
    expect(createMaaAutomationMock).not.toHaveBeenCalled();
    await user.click(screen.getByRole('checkbox', { name: /^我确认无人值守运行 MAA/ }));
    await user.click(createButton);
    await waitFor(() => expect(createMaaAutomationMock).toHaveBeenCalledWith({
      title: 'MAA 日常队列', triggerValue: '每天 18:00', workspaceId: 'project-a',
      appId: 'maa-app', launchEmulator: false, timeoutSeconds: 3600,
    }));
    expect(createAutomationMock).not.toHaveBeenCalled();
  });

  it('guides missing MAA trust to existing settings without enabling creation', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);
    await user.selectOptions(screen.getByRole('combobox', { name: '任务类型' }), 'maa');
    await user.type(screen.getByLabelText('任务名称'), 'MAA 日常队列');
    expect(screen.getByText(/设置 → 电脑操作 → 受信任应用/)).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: /^受信任的 MAA 应用/ })).toBeDisabled();
    expect(screen.getByRole('button', { name: '预览规则' })).toBeDisabled();
    expect(createMaaAutomationMock).not.toHaveBeenCalled();
  });

  it('requires renewed MAA confirmation after an edit and sends emulator launch only when opted in', async () => {
    const user = userEvent.setup();
    getDesktopTrustedAppsMock.mockResolvedValue([trustedMaaApp]);
    render(<AutomationPage />);
    await user.selectOptions(screen.getByRole('combobox', { name: '任务类型' }), 'maa');
    await screen.findByRole('option', { name: '测试 MAA' });
    await user.type(screen.getByLabelText('任务名称'), 'MAA 日常队列');
    await user.click(screen.getByRole('checkbox', { name: /^按 MAA 当前配置启动模拟器/ }));
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    await user.click(screen.getByRole('checkbox', { name: /^我确认无人值守运行 MAA/ }));
    const timing = screen.getByRole('textbox', { name: /^触发时间/ });
    await user.clear(timing);
    await user.type(timing, '19:45');
    expect(screen.queryByRole('button', { name: '确认并创建 MAA 任务' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    const confirmation = screen.getByRole('checkbox', { name: /^我确认无人值守运行 MAA/ });
    expect(confirmation).not.toBeChecked();
    await user.click(confirmation);
    await user.click(screen.getByRole('button', { name: '确认并创建 MAA 任务' }));
    await waitFor(() => expect(createMaaAutomationMock).toHaveBeenCalledWith(expect.objectContaining({
      triggerValue: '每天 19:45', workspaceId: 'personal', launchEmulator: true,
    })));
  });

  it('surfaces a MAA creation refusal without claiming creation succeeded', async () => {
    const user = userEvent.setup();
    getDesktopTrustedAppsMock.mockResolvedValue([trustedMaaApp]);
    createMaaAutomationMock.mockRejectedValueOnce('MAA 应用不再受信任，请重新选择');
    render(<AutomationPage />);
    await user.selectOptions(screen.getByRole('combobox', { name: '任务类型' }), 'maa');
    await screen.findByRole('option', { name: '测试 MAA' });
    await user.type(screen.getByLabelText('任务名称'), 'MAA 日常队列');
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    await user.click(screen.getByRole('checkbox', { name: /^我确认无人值守运行 MAA/ }));
    await user.click(screen.getByRole('button', { name: '确认并创建 MAA 任务' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('MAA 应用不再受信任，请重新选择');
    expect(screen.getByRole('button', { name: '确认并创建 MAA 任务' })).toBeEnabled();
    expect(getAutomationsMock).toHaveBeenCalledTimes(1);
  });

  it.each([
    ['running', '已启动，等待结果', 'MAA 启动已登记'],
    ['failed', '存在失败', '队列结束，部分任务失败'],
    ['attention_required', '需要处理', 'MAA 队列结果未知'],
    ['completed', '队列已结束', 'MAA 当前队列已结束'],
  ])('presents MAA %s without equating queue status with all game goals', async (status, label, summary) => {
    getAutomationsMock.mockResolvedValue([{ ...maaAutomation, enabled: status !== 'attention_required' }]);
    getAutomationRunsMock.mockResolvedValue([
      { id: 'skip', status: 'skipped', summary: '已有运行，跳过此次调度', output: '', startedAt: 2 },
      { id: 'queue', status, summary, output: '', startedAt: 1 },
    ]);
    render(<AutomationPage />);
    expect(await screen.findByText(label)).toBeInTheDocument();
    expect(screen.getByText(summary)).toBeInTheDocument();
    expect(screen.getByText('MAA 当前任务队列 · 项目 A')).toBeInTheDocument();
    if (status === 'running') {
      expect(screen.getByRole('button', { name: '等待结果' })).toBeDisabled();
      expect(screen.getByRole('button', { name: '删除 MAA 日常队列' })).toBeDisabled();
      expect(screen.getByRole('button', { name: '暂停' })).toBeEnabled();
      expect(screen.getByText(/不能仅凭此回执确认MAA 队列仍在运行/)).toBeInTheDocument();
    } else if (status === 'attention_required') {
      expect(screen.getByRole('button', { name: '核对后重新运行' })).toBeEnabled();
      expect(screen.getByRole('button', { name: '启用' })).toBeDisabled();
      expect(screen.getByText(/不会自动重试/)).toBeInTheDocument();
    } else if (status === 'completed') {
      expect(screen.getByText('记录的是 MAA 队列执行结果，不代表游戏业务目标全部完成。')).toBeInTheDocument();
    }
    expect(runAutomationNowMock).not.toHaveBeenCalled();
  });
});
