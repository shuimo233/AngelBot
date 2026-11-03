import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { createAutomation, createMaaAutomation, deleteAutomation, getAutomationRuns, getAutomationDashboard, runAutomationNow, setAutomationEnabled, stopAutomationRun, type Automation, type AutomationRun } from '$lib/commands/automation';
import { getDesktopTrustedApps } from '$lib/commands/desktop';
import { AUTOMATIONS_UPDATED_EVENT } from '$lib/automation-events';
import { getWorkspaces } from '$lib/commands/workspace';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';
import { AutomationPage } from './AutomationPage';
import { getAutostart, setAutostart } from '$lib/commands/settings';
import { open } from '@tauri-apps/plugin-dialog';

vi.mock('$lib/commands/settings', () => ({ getAutostart: vi.fn(), setAutostart: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

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
// Creation is deliberately secondary to the task list. Exercise the visible
// disclosure before querying fields; do not force test-only component state.
function openCreation() {
  const create = screen.queryByRole('button', { name: '添加自动化' });
  if (create) fireEvent.click(create);
}

function taskNameInput() {
  openCreation();
  return screen.getByLabelText('任务名称');
}

describe('AutomationPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getAutostart).mockResolvedValue(false);
    vi.mocked(setAutostart).mockResolvedValue(undefined);
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
    vi.mocked(open).mockResolvedValue(null);
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

  it('uses a conversation hint, not a second natural-language configuration flow, and preserves the manual draft', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);
    await screen.findByText(/可以直接与 AngelBot 讨论/);
    expect(screen.queryByRole('button', { name: '解析' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '一句话创建' })).not.toBeInTheDocument();
    expect(screen.queryByLabelText('任务名称')).not.toBeInTheDocument();
    expect(getDesktopTrustedAppsMock).not.toHaveBeenCalled();
    await user.type(taskNameInput(), '整理日常文件');
    expect(screen.getByRole('heading', { name: '基本信息' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: '脚本与参数' })).toBeInTheDocument();
    expect(screen.getByText('运行选项').closest('details')).not.toHaveAttribute('open');
    expect(screen.getByRole('button', { name: '选择目录' })).not.toBeVisible();
    expect(screen.queryByRole('combobox', { name: '任务类型' })).not.toBeInTheDocument();
    expect(screen.queryByRole('option', { name: 'MAA 当前任务队列' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '收起配置' }));
    await user.click(screen.getByRole('button', { name: '添加自动化' }));
    expect(screen.getByLabelText('任务名称')).toHaveValue('整理日常文件');
    await user.click(screen.getByRole('button', { name: '去对话' }));
    expect(useNavigationStore.getState().currentPage).toBe('chat');
    expect(useWorkspacesStore.getState().openWorkspace).not.toHaveBeenCalled();
    expect(createAutomationMock).not.toHaveBeenCalled();
    expect(createMaaAutomationMock).not.toHaveBeenCalled();
  });
  it('shows task summaries first and opens actions only through the shared disclosure', async () => {
    const user = userEvent.setup();
    getAutomationsMock.mockResolvedValue([scriptAutomation]);
    render(<AutomationPage />);
    const summary = await screen.findByLabelText('现有脚本 详情');
    const details = summary.closest('details')!;
    expect(details).not.toHaveAttribute('open');
    expect(getHistoryMock).not.toHaveBeenCalled();
    await user.click(summary);
    expect(details).toHaveAttribute('open');
    expect(screen.getByRole('button', { name: '立即运行' })).toBeVisible();
    getAutomationsMock.mockResolvedValue([{ ...scriptAutomation, enabled: false }]);
    fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT));
    await waitFor(() => expect(summary).toHaveTextContent('已暂停'));
    expect(details).toHaveAttribute('open');
    await user.click(summary);
    expect(details).not.toHaveAttribute('open');
    expect(runAutomationNowMock).not.toHaveBeenCalled();
    expect(deleteAutomationMock).not.toHaveBeenCalled();
  });

  it('selects generic scripts and working directories while preserving a cancelled selection and surfacing picker failures', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);
    await user.type(taskNameInput(), '整理文件');
    expect(screen.getByText('保存副本运行，原文件不变。')).toBeVisible();
    const details = screen.getByText('脚本使用说明').closest('details')!;
    expect(details).not.toHaveAttribute('open');
    const copyDetails = screen.getByText(/源文件更新不会自动同步/);
    expect(copyDetails).not.toBeVisible();
    await user.click(screen.getByText('脚本使用说明'));
    expect(copyDetails).toBeVisible();
    await user.click(screen.getByText('脚本使用说明'));
    vi.mocked(open).mockResolvedValueOnce('C:/approved/report.ps1');
    await user.click(screen.getByRole('button', { name: '选择脚本' }));
    expect(screen.getByLabelText('脚本路径')).toHaveValue('C:/approved/report.ps1');
    expect(open).toHaveBeenLastCalledWith(expect.objectContaining({ directory: false, multiple: false, defaultPath: undefined, filters: [{ name: '本地脚本', extensions: ['py', 'js', 'mjs', 'cjs', 'ps1', 'bat', 'cmd'] }] }));
    await user.click(screen.getByText('运行选项'));
    vi.mocked(open).mockResolvedValueOnce('C:/approved/reports');
    await user.click(screen.getByRole('button', { name: '选择目录' }));
    expect(screen.getByLabelText('脚本工作目录（可选）')).toHaveValue('C:/approved/reports');
    await user.click(screen.getByText('运行选项'));
    expect(screen.getByRole('button', { name: '选择目录' })).not.toBeVisible();
    expect(screen.getByText(/自定义工作目录 · 300 秒超时/)).toBeVisible();
    await user.click(screen.getByText('运行选项'));
    expect(screen.getByLabelText('脚本工作目录（可选）')).toHaveValue('C:/approved/reports');
    await user.click(screen.getByRole('button', { name: '选择脚本' }));
    expect(screen.getByLabelText('脚本路径')).toHaveValue('C:/approved/report.ps1');
    vi.mocked(open).mockRejectedValueOnce(new Error('选择器暂不可用'));
    await user.click(screen.getByRole('button', { name: '选择脚本' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('选择器暂不可用');
    expect(createAutomationMock).not.toHaveBeenCalled();
    expect(createMaaAutomationMock).not.toHaveBeenCalled();
  });

  it('previews an arbitrary source as a managed copy and invalidates review after edits', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);
    await user.type(taskNameInput(), '日报');
    await user.type(screen.getByLabelText('脚本路径'), 'D:/external/report.py');
    expect(screen.queryByText(/脚本路径范围/)).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    expect(screen.getByRole('status')).toHaveTextContent('D:/external/report.py');
    expect(screen.getByRole('status')).toHaveTextContent('AngelBot 托管目录');
    expect(screen.getByRole('status')).toHaveTextContent('只运行可信脚本');
    expect(screen.getByRole('status')).toHaveTextContent('不含依赖');
    expect(screen.getByRole('status')).toHaveTextContent('300 秒');
    await user.type(screen.getByLabelText('任务名称'), '（修改）');
    expect(screen.queryByRole('button', { name: '创建任务' })).not.toBeInTheDocument();
    expect(createAutomationMock).not.toHaveBeenCalled();
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
    expect(card.querySelector('.automation-card-details')).toHaveAttribute('open');
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

  it('stages clock edits in an overlay and preserves the form on cancel or Escape', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);

    await user.type(taskNameInput(), '生成日报');
    const trigger = screen.getByRole('button', { name: '选择执行时间' });
    expect(screen.queryByRole('textbox', { name: '每天执行时间' })).not.toBeInTheDocument();
    await user.click(trigger);
    const dialog = screen.getByRole('dialog', { name: '选择执行时间' });
    expect(dialog.closest('.automation-page')).toBeNull();
    expect(within(dialog).getByRole('slider', { name: '小时表盘' })).toHaveAttribute('aria-valuenow', '9');
    await user.click(within(dialog).getByRole('button', { name: '18 时' }));
    expect(within(dialog).getByRole('slider', { name: '分钟表盘' })).toBeInTheDocument();
    expect(screen.getByRole('group', { name: '每天执行时间' })).toHaveTextContent('09:00');
    await user.click(within(dialog).getByRole('button', { name: '取消' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    await user.click(trigger);
    expect(screen.getByRole('slider', { name: '小时表盘' })).toHaveAttribute('aria-valuenow', '9');
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByRole('group', { name: '每天执行时间' })).toHaveTextContent('09:00');
    expect(createAutomationMock).not.toHaveBeenCalled();
  });

  it('previews and creates a script only with the canonical daily schedule', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);

    await user.type(taskNameInput(), '生成日报');
    await user.click(screen.getByRole('button', { name: '选择执行时间' }));
    await user.click(screen.getByRole('button', { name: '09 时' }));
    await user.click(screen.getByRole('button', { name: '30 分' }));
    // Exact minutes remain available without a second text input.
    const minutes = screen.getByRole('slider', { name: '分钟表盘' });
    fireEvent.keyDown(minutes, { key: 'ArrowRight' });
    expect(minutes).toHaveAttribute('aria-valuenow', '31');
    fireEvent.keyDown(minutes, { key: 'ArrowLeft' });
    await user.click(screen.getByRole('button', { name: '确定' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByRole('group', { name: '每天执行时间' })).toHaveTextContent('09:30');
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

    await user.type(taskNameInput(), '运行现有流程');
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

  it('switches to the existing Main executor without saving hidden script settings', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);
    await user.type(taskNameInput(), '每日事项汇总');
    await user.type(screen.getByLabelText('脚本路径'), 'scripts/old.py');
    await user.selectOptions(screen.getByLabelText(/^是否使用模型/), 'agent');
    expect(screen.queryByLabelText('脚本路径')).not.toBeInTheDocument();
    expect(screen.getByLabelText(/^归属空间/)).toHaveValue('personal');
    expect(screen.getByRole('button', { name: '预览规则' })).toBeDisabled();
    await user.type(screen.getByLabelText('任务说明'), '汇总今天待处理的事项，给出简短清单');
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    expect(screen.getByRole('status')).toHaveTextContent('是 · 当前已保存的模型');
    await user.click(screen.getByRole('button', { name: '创建任务' }));
    await waitFor(() => expect(createAutomationMock).toHaveBeenCalledWith(expect.objectContaining({
      executorKind: 'agent', workspaceId: 'personal', prompt: '汇总今天待处理的事项，给出简短清单',
      scriptPath: undefined, scriptArgs: [], workingDir: undefined,
    })));
    expect(setAutostart).not.toHaveBeenCalled();
    expect(runAutomationNowMock).not.toHaveBeenCalled();
  });

  it('requires explicit startup registration and never executes while creating a startup task', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);
    await user.type(taskNameInput(), '登录后整理');
    await user.type(screen.getByLabelText('脚本路径'), 'scripts/daily.ps1');
    await user.selectOptions(screen.getByRole('combobox', { name: /^触发方式/ }), 'startup');
    expect(screen.queryByLabelText('每天执行时间')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '预览规则' })).toBeDisabled();
    expect(setAutostart).not.toHaveBeenCalled();
    vi.mocked(setAutostart).mockRejectedValueOnce('Windows 拒绝注册');
    await user.click(await screen.findByRole('button', { name: '启用 AngelBot 开机启动' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Windows 拒绝注册');
    expect(screen.getByRole('button', { name: '预览规则' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: '启用 AngelBot 开机启动' }));
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    expect(screen.getByRole('status')).toHaveTextContent('从下次 Windows 登录开始触发');
    await user.selectOptions(screen.getByRole('combobox', { name: /^触发方式/ }), 'schedule');
    expect(screen.queryByRole('button', { name: '创建任务' })).not.toBeInTheDocument();
    vi.mocked(getAutostart).mockResolvedValue(true);
    await user.selectOptions(screen.getByRole('combobox', { name: /^触发方式/ }), 'startup');
    await waitFor(() => expect(screen.getByRole('button', { name: '预览规则' })).toBeEnabled());
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    await user.click(screen.getByRole('button', { name: '创建任务' }));
    await waitFor(() => expect(createAutomationMock).toHaveBeenCalledWith(expect.objectContaining({
      triggerKind: 'startup', triggerValue: '登录 Windows 后', executorKind: 'script',
    })));
    expect(setAutostart).toHaveBeenCalledWith(true);
    expect(runAutomationNowMock).not.toHaveBeenCalled();
  });

  it('shows native batch validation errors without claiming the script was saved', async () => {
    const user = userEvent.setup();
    const reason = '批处理脚本路径和参数不能包含 shell 元字符、引号或控制字符';
    createAutomationMock.mockRejectedValueOnce(reason);
    render(<AutomationPage />);
    await user.type(taskNameInput(), '现有脚本');
    await user.type(screen.getByLabelText('脚本路径'), 'scripts/daily.cmd');
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    expect(screen.getByText(/脚本结束或超时会回收其自有进程树/)).toBeInTheDocument();
    expect(screen.getByText(/批处理（.bat\/\.cmd）不接受引号或 shell 元字符/)).toBeInTheDocument();
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
    expect(screen.getByText('定时任务需保持 AngelBot 运行。')).toBeVisible();
    openCreation();
    expect(screen.getByText(/退出码不保证外部业务完成/)).not.toBeVisible();
    await userEvent.click(screen.getByText('脚本使用说明'));
    expect(screen.getByText(/退出码不保证外部业务完成/)).toBeVisible();
    expect(screen.getByText(/GUI 辅助工具需要明确的任务控制接口/)).toBeInTheDocument();
    expect(screen.getByText(/仅启动软件后退出的脚本不代表其中任务已执行/)).toBeInTheDocument();
    expect(screen.queryByText(/GUI 辅助工具优先使用已有定时功能/)).not.toBeInTheDocument();
    expect(screen.getByText(/暂停仅影响后续调度，不会停止已启动的脚本/)).toBeInTheDocument();

    const loadCount = getAutomationsMock.mock.calls.length;
    unmount();
    fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT));
    expect(getAutomationsMock).toHaveBeenCalledTimes(loadCount);
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
