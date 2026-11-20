import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { createAutomation, createMaaAutomation, deleteAutomation, getAutomationRuns, getAutomationDashboard, runAutomationNow, setAutomationEnabled, stopAutomationRun, type Automation, type AutomationRun, type ManualAutomationStart } from '$lib/commands/automation';
import { getDesktopTrustedApps, type TrustedDesktopApp } from '$lib/commands/desktop';
import { AUTOMATIONS_UPDATED_EVENT } from '$lib/automation-events';
import { getWorkspaces } from '$lib/commands/workspace';
import { useNavigationStore } from '$stores/navigation';
import { useMessagesStore } from '$stores/messages';
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
const trustedMaa: TrustedDesktopApp = {
  id: 'trusted-maa', displayName: '我的 MAA', executablePath: 'C:\\Tools\\Maa.exe', capabilities: ['launch'],
  draftSelector: null, enabled: true, createdAt: 1, updatedAt: 2,
};
// Creation is deliberately secondary to the task list. Exercise the visible
// disclosure before querying fields; do not force test-only component state.
function openCreation(executor: 'script' | 'agent' | 'maa' = 'script') {
  const create = screen.queryByRole('button', { name: '添加自动化' });
  if (create) fireEvent.click(create);
  if (executor === 'script') fireEvent.click(screen.getByRole('radio', { name: '运行本地脚本' }));
  if (executor === 'maa') fireEvent.click(screen.getByRole('radio', { name: '明日方舟日常（MAA）' }));
}

function taskNameInput() {
  openCreation();
  return screen.getByLabelText('任务名称');
}

describe('AutomationPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sessionStorage.clear();
    vi.mocked(getAutostart).mockResolvedValue(false);
    vi.mocked(setAutostart).mockResolvedValue(undefined);
    getAutomationsMock.mockResolvedValue([]);
    getAutomationRunsMock.mockResolvedValue([]);
    getHistoryMock.mockResolvedValue([]);
    getDashboardMock.mockImplementation(async (pendingRequests = []) => {
      const items = await Promise.all((await getAutomationsMock()).map(async item => {
      const runs = await getAutomationRunsMock(item.id);
      const matched = pendingRequests.find(request => request.automationId === item.id
        && runs.some(run => run.id === request.runId));
      return { item, runs, matched };
      }));
      return { items: items.map(({ item, runs }) => {
      const eligible = runs.filter(run => run.status !== 'skipped' || item.executorKind === 'notification');
      const latestRun = eligible.find(run => run.status === 'running')
        ?? eligible.find(run => ['queued', 'awaiting_confirmation'].includes(run.status)) ?? eligible[0] ?? null;
      return { ...item, latestRun, lastSkippedAt: runs[0]?.status === 'skipped' ? runs[0].startedAt : null };
      }), acceptedRequests: items.flatMap(({ matched }) => matched ? [matched] : []) };
    });
    getDesktopTrustedAppsMock.mockResolvedValue([]);
    vi.mocked(open).mockResolvedValue(null);
    createMaaAutomationMock.mockResolvedValue(maaAutomation);
    runAutomationNowMock.mockResolvedValue({ status: 'accepted' });
    setAutomationEnabledMock.mockResolvedValue(undefined);
    stopAutomationRunMock.mockResolvedValue(undefined);
    deleteAutomationMock.mockResolvedValue(undefined);
    getWorkspacesMock.mockResolvedValue([
      { id: 'personal', name: 'AngelBot 日常', kind: 'personal', createdAt: 1, updatedAt: 1, activeSessionId: 'personal-main' },
      { id: 'project-a', name: '项目 A', kind: 'project', rootPath: 'C:/project-a', createdAt: 1, updatedAt: 1, activeSessionId: 'project-a-main' },
    ]);
    useNavigationStore.setState({ currentPage: 'tasks', automationTarget: null });
    useMessagesStore.setState({ input: '', inputSessionId: null, inputToken: {}, draftInputs: {} });
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

  it('links to the conversation and preserves the manual draft', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);
    expect(screen.getByRole('button', { name: '在对话里交代' })).toBeVisible();
    expect(screen.queryByRole('button', { name: '解析' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '一句话创建' })).not.toBeInTheDocument();
    expect(screen.queryByLabelText('任务名称')).not.toBeInTheDocument();
    expect(getDesktopTrustedAppsMock).not.toHaveBeenCalled();
    await user.type(taskNameInput(), '整理日常文件');
    expect(screen.getByRole('heading', { name: '要做什么' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: '脚本与参数' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '运行选项' })).toHaveAttribute('aria-haspopup', 'dialog');
    expect(screen.queryByRole('button', { name: '选择目录' })).not.toBeInTheDocument();
    expect(screen.queryByLabelText('参数（每行一个）')).not.toBeInTheDocument();
    expect(screen.queryByRole('combobox', { name: '任务类型' })).not.toBeInTheDocument();
    expect(screen.getByRole('radio', { name: '明日方舟日常（MAA）' })).toBeVisible();
    expect(screen.queryByLabelText('可信 MAA')).not.toBeInTheDocument();
    expect(getDesktopTrustedAppsMock).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: '返回任务' }));
    await user.click(screen.getByRole('button', { name: '添加自动化' }));
    expect(screen.getByLabelText('任务名称')).toHaveValue('整理日常文件');
    await user.type(screen.getByLabelText('脚本路径'), 'C:/Tools/daily.ps1');
    screen.getByRole('button', { name: '预览规则' }).focus();
    await user.keyboard('{Enter}');
    expect(screen.queryByRole('group', { name: '自动化配置' })).not.toBeInTheDocument();
    expect(screen.getByRole('heading', { name: '确认创建任务' })).toBeVisible();
    expect(screen.getByRole('heading', { name: '确认创建任务' })).toHaveFocus();
    screen.getByRole('button', { name: '返回编辑' }).focus();
    await user.keyboard('{Enter}');
    expect(screen.getByRole('heading', { name: '添加自动化' })).toHaveFocus();
    expect(screen.getByLabelText('任务名称')).toHaveValue('整理日常文件');
    expect(screen.getByLabelText('脚本路径')).toHaveValue('C:/Tools/daily.ps1');
    const titleInput = screen.getByLabelText('任务名称');
    await user.type(titleInput, '（修订）');
    expect(titleInput).toHaveFocus();
    const readCount = getDashboardMock.mock.calls.length;
    fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT));
    await waitFor(() => expect(getDashboardMock).toHaveBeenCalledTimes(readCount + 1));
    expect(titleInput).toHaveFocus();
    await user.click(screen.getByRole('button', { name: '返回任务' }));
    await user.click(screen.getByRole('button', { name: '在对话里交代' }));
    expect(useNavigationStore.getState().currentPage).toBe('chat');
    expect(useWorkspacesStore.getState().openWorkspace).not.toHaveBeenCalled();
    expect(createAutomationMock).not.toHaveBeenCalled();
    expect(createMaaAutomationMock).not.toHaveBeenCalled();
  });

  it('saves one daily MAA schedule without a model or starting the queue', async () => {
    const user = userEvent.setup();
    const nextRunAt = 1_793_452_800;
    const persisted = { ...maaAutomation, title: '明日方舟日常', nextRunAt, lastRunAt: undefined };
    let finishSave!: (item: Automation) => void;
    createMaaAutomationMock.mockImplementationOnce(() => new Promise(resolve => { finishSave = resolve; }));
    getDesktopTrustedAppsMock.mockResolvedValue([trustedMaa, { ...trustedMaa, id: 'other', displayName: '别的程序', executablePath: 'C:/Apps/not-maa.exe' }]);
    render(<AutomationPage />);
    openCreation('maa');
    await screen.findByRole('option', { name: '我的 MAA' });
    expect(screen.queryByRole('option', { name: '别的程序' })).not.toBeInTheDocument();
    expect(screen.queryByRole('radio', { name: '登录 Windows 时' })).not.toBeInTheDocument();
    expect(screen.queryByLabelText('任务说明')).not.toBeInTheDocument();
    expect(screen.getByText(/勾选的任务队列.*不需要连接模型/)).toBeVisible();

    const save = screen.getByRole('button', { name: '保存 MAA 日程' });
    expect(save).toBeDisabled();
    await user.selectOptions(screen.getByLabelText('可信 MAA'), 'trusted-maa');
    await user.selectOptions(screen.getByLabelText('归属空间'), 'project-a');
    const simulator = screen.getByRole('checkbox', { name: '按需启动 MAA 已配置的模拟器' });
    expect(simulator).not.toBeChecked();
    await user.click(simulator);
    await user.click(screen.getByRole('button', { name: '选择执行时间' }));
    await user.click(screen.getByRole('button', { name: '18 时' }));
    await user.click(screen.getByRole('button', { name: '00 分' }));
    await user.click(screen.getByRole('button', { name: '确定' }));
    act(() => { fireEvent.click(save); fireEvent.click(save); });
    expect(screen.getByRole('button', { name: '保存中…' })).toBeDisabled();
    expect(createMaaAutomationMock).toHaveBeenCalledTimes(1);
    expect(createMaaAutomationMock).toHaveBeenCalledWith({ title: '明日方舟日常', triggerValue: '每天 18:00', workspaceId: 'project-a', appId: 'trusted-maa', launchEmulator: true });
    getAutomationsMock.mockResolvedValue([persisted]);
    await act(async () => finishSave(persisted));

    const item = await screen.findByRole('article', { name: '明日方舟日常' });
    expect(screen.getAllByRole('article', { name: '明日方舟日常' })).toHaveLength(1);
    expect(item).toHaveTextContent(/每天 18:00.*MAA 当前任务队列 · 项目 A/);
    expect(within(item).getByText('下次执行').nextElementSibling).toHaveTextContent(new Date(nextRunAt * 1000).toLocaleString());
    expect(createAutomationMock).not.toHaveBeenCalled();
    expect(runAutomationNowMock).not.toHaveBeenCalled();
    expect(setAutostart).not.toHaveBeenCalled();
  });

  it('leaves MAA creation with no side effects and preserves the draft', async () => {
    getDesktopTrustedAppsMock.mockResolvedValue([trustedMaa]);
    render(<AutomationPage />);
    openCreation('maa');
    await screen.findByRole('option', { name: '我的 MAA' });
    fireEvent.change(screen.getByLabelText('可信 MAA'), { target: { value: 'trusted-maa' } });
    fireEvent.change(screen.getByLabelText('任务名称（选填）'), { target: { value: '我的日程' } });
    fireEvent.click(screen.getByRole('button', { name: '返回任务' }));
    expect(await screen.findByText('暂无自动化任务')).toBeVisible();
    expect(createMaaAutomationMock).not.toHaveBeenCalled();
    expect(runAutomationNowMock).not.toHaveBeenCalled();
    openCreation('maa');
    expect(await screen.findByRole('option', { name: '我的 MAA' })).toBeInTheDocument();
    expect(screen.getByLabelText('任务名称（选填）')).toHaveValue('我的日程');
  });

  it('blocks missing and disabled trusted MAA apps and leaves native revalidation authoritative', async () => {
    getDesktopTrustedAppsMock.mockResolvedValueOnce([]).mockResolvedValueOnce([{ ...trustedMaa, enabled: false }]).mockResolvedValue([trustedMaa]);
    createMaaAutomationMock.mockRejectedValueOnce('MAA 已停用或未授予打开应用权限');
    render(<AutomationPage />);
    openCreation('maa');
    expect(await screen.findByText('尚未添加可信的 MAA.exe。')).toBeVisible();
    const save = screen.getByRole('button', { name: '保存 MAA 日程' });
    expect(save).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: '重新读取可信应用' }));
    expect(await screen.findByRole('option', { name: '我的 MAA（已停用）', hidden: true })).toBeDisabled();
    expect(screen.getByText('可信列表中的 MAA 已停用或未授予打开应用权限。')).toBeVisible();
    expect(save).toBeDisabled();
    expect(createMaaAutomationMock).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: '重新读取可信应用' }));
    await screen.findByRole('option', { name: '我的 MAA' });
    fireEvent.change(screen.getByLabelText('可信 MAA'), { target: { value: 'trusted-maa' } });
    await waitFor(() => expect(save).toBeEnabled());
    fireEvent.click(save);
    expect(await screen.findByRole('alert')).toHaveTextContent('MAA 已停用或未授予打开应用权限');
    expect(screen.getByRole('heading', { name: '添加自动化' })).toBeVisible();
    expect(runAutomationNowMock).not.toHaveBeenCalled();
  });
  it('shows task summaries first and opens actions only through the shared disclosure', async () => {
    const user = userEvent.setup();
    getAutomationsMock.mockResolvedValue([scriptAutomation]);
    render(<AutomationPage />);
    const summary = await screen.findByLabelText('现有脚本 详情');
    const details = summary.closest('details')!;
    expect(details).not.toHaveAttribute('open');
    expect(summary).toHaveAccessibleDescription(/下次执行.*最近结果.*尚无运行记录/);
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
    const instructions = screen.getByRole('button', { name: '脚本使用说明' });
    expect(screen.queryByText(/源文件更新不会自动同步/)).not.toBeInTheDocument();
    await user.click(instructions);
    expect(screen.getByRole('dialog', { name: '脚本使用说明' })).toHaveTextContent(/源文件更新不会自动同步/);
    await user.keyboard('{Escape}');
    expect(instructions).toHaveFocus();
    vi.mocked(open).mockResolvedValueOnce('C:/approved/report.ps1');
    await user.click(screen.getByRole('button', { name: '选择脚本' }));
    expect(screen.getByLabelText('脚本路径')).toHaveValue('C:/approved/report.ps1');
    expect(open).toHaveBeenLastCalledWith(expect.objectContaining({ directory: false, multiple: false, defaultPath: undefined, filters: [{ name: '本地脚本', extensions: ['py', 'js', 'mjs', 'cjs', 'ps1', 'bat', 'cmd'] }] }));
    const options = screen.getByRole('button', { name: '运行选项' });
    await user.click(options);
    vi.mocked(open).mockResolvedValueOnce('C:/approved/reports');
    await user.click(screen.getByRole('button', { name: '选择目录' }));
    expect(screen.getByLabelText('脚本工作目录（可选）')).toHaveValue('C:/approved/reports');
    await user.click(screen.getByRole('button', { name: '返回配置' }));
    expect(options).toHaveFocus();
    expect(screen.queryByRole('button', { name: '选择目录' })).not.toBeInTheDocument();
    expect(screen.getByText(/自定义工作目录 · 300 秒超时/)).toBeVisible();
    await user.click(options);
    expect(screen.getByLabelText('脚本工作目录（可选）')).toHaveValue('C:/approved/reports');
    await user.click(screen.getByRole('button', { name: '返回配置' }));
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
    await user.click(screen.getByRole('button', { name: '返回编辑' }));
    await user.type(screen.getByLabelText('任务名称'), '（修改）');
    expect(screen.queryByRole('button', { name: '创建任务' })).not.toBeInTheDocument();
    expect(createAutomationMock).not.toHaveBeenCalled();
  });

  it('uses one dashboard read and lazily loads history with explicit failure retry', async () => {
    getDashboardMock.mockResolvedValue({ acceptedRequests: [], items: [{ ...scriptAutomation, latestRun: {
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
    const payload = { acceptedRequests: [], items: [{ ...scriptAutomation, latestRun: null, lastSkippedAt: null }] };
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
    fireEvent.click(screen.getByRole('button', { name: '添加自动化' }));
    fireEvent.change(screen.getByLabelText('任务名称'), { target: { value: '保留中的草稿' } });
    act(() => useNavigationStore.getState().openAutomation({ automationId: scriptAutomation.id, workspaceId: 'project-a' }));
    await waitFor(() => expect(screen.getByRole('article', { name: '现有脚本' })).toHaveFocus());
    fireEvent.click(screen.getByRole('button', { name: '添加自动化' }));
    expect(screen.getByLabelText('任务名称')).toHaveValue('保留中的草稿');
    expect(runAutomationNowMock).not.toHaveBeenCalled();
    expect(stopAutomationRunMock).not.toHaveBeenCalled();
  });

  it('ignores a closed history request and keeps an owner-pinned unknown head authoritative', async () => {
    let finishHistory!: (runs: AutomationRun[]) => void;
    getHistoryMock.mockReturnValueOnce(new Promise(resolve => { finishHistory = resolve; })).mockResolvedValueOnce([
      { id: 'current-history', status: 'failed', summary: '本次读取的记录', output: '', startedAt: 3 },
    ]);
    getDashboardMock.mockResolvedValue({ acceptedRequests: [], items: [{ ...scriptAutomation, enabled: false, latestRun: {
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
    await user.click(screen.getByText('运行选项'));
    fireEvent.change(screen.getByRole('textbox', { name: /^参数/ }), {
      target: { value: '--profile\r\nC:\\Game Tools\\daily.json\r\n\r\n  spaced argument  \r\n"literal quotes"' },
    });
    await user.click(screen.getByRole('button', { name: '返回配置' }));
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
    await user.click(screen.getByRole('radio', { name: '交给 AngelBot' }));
    expect(screen.queryByLabelText('脚本路径')).not.toBeInTheDocument();
    expect(screen.getByLabelText('结果发到')).toHaveValue('personal');
    expect(screen.getByRole('button', { name: '预览规则' })).toBeDisabled();
    await user.type(screen.getByLabelText('任务说明'), '汇总今天待处理的事项，给出简短清单');
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    expect(screen.getByRole('status')).toHaveTextContent('是 · 当前已保存的模型');
    expect(screen.getByRole('status')).toHaveTextContent('预计首次运行');
    expect(screen.getByRole('status')).toHaveTextContent('查看结果');
    expect(screen.getByRole('status')).toHaveTextContent('到点时 AngelBot 需保持运行');
    await user.click(screen.getByRole('button', { name: '创建任务' }));
    await waitFor(() => expect(createAutomationMock).toHaveBeenCalledWith(expect.objectContaining({
      executorKind: 'agent', workspaceId: 'personal', prompt: '汇总今天待处理的事项，给出简短清单',
      scriptPath: undefined, scriptArgs: [], workingDir: undefined,
    })));
    expect(setAutostart).not.toHaveBeenCalled();
    expect(runAutomationNowMock).not.toHaveBeenCalled();
  });

  it('starts with a task for AngelBot and keeps script setup out of the way', () => {
    render(<AutomationPage />);
    openCreation('agent');
    expect(screen.getByRole('radio', { name: '交给 AngelBot' })).toBeChecked();
    expect(screen.getByLabelText('任务说明')).toBeVisible();
    expect(screen.queryByLabelText('脚本路径')).not.toBeInTheDocument();
    expect(createAutomationMock).not.toHaveBeenCalled();
  });

  it('offers a reminder draft in the current conversation and preserves an existing draft', async () => {
    const workspaces = await getWorkspaces();
    useWorkspacesStore.setState({ workspaces });
    render(<AutomationPage />);
    const goToChat = screen.getByRole('button', { name: '在对话里交代' });
    await userEvent.click(goToChat);
    expect(useMessagesStore.getState()).toMatchObject({ inputSessionId: 'personal-main', input: '帮我设一个提醒：' });
    useMessagesStore.getState().setInput('我的未发送草稿');
    await userEvent.click(goToChat);
    expect(useMessagesStore.getState().input).toBe('我的未发送草稿');
  });

  it('requires explicit startup registration and never executes while creating a startup task', async () => {
    const user = userEvent.setup();
    render(<AutomationPage />);
    await user.type(taskNameInput(), '登录后整理');
    await user.type(screen.getByLabelText('脚本路径'), 'scripts/daily.ps1');
    await user.click(screen.getByRole('radio', { name: '登录 Windows 时' }));
    expect(screen.queryByLabelText('每天执行时间')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '预览规则' })).toBeDisabled();
    expect(setAutostart).not.toHaveBeenCalled();
    vi.mocked(setAutostart).mockRejectedValueOnce('Windows 拒绝注册');
    await user.click(await screen.findByRole('button', { name: '启用 AngelBot 开机启动' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Windows 拒绝注册');
    expect(screen.getByRole('button', { name: '预览规则' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: '启用 AngelBot 开机启动' }));
    await user.click(screen.getByRole('button', { name: '预览规则' }));
    expect(screen.getByRole('status')).toHaveTextContent('下次登录 Windows 时触发');
    await user.click(screen.getByRole('button', { name: '返回编辑' }));
    await user.click(screen.getByRole('radio', { name: '每天定时' }));
    expect(screen.queryByRole('button', { name: '创建任务' })).not.toBeInTheDocument();
    vi.mocked(getAutostart).mockResolvedValue(true);
    await user.click(screen.getByRole('radio', { name: '登录 Windows 时' }));
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
    await user.click(screen.getByRole('button', { name: '脚本使用说明' }));
    expect(screen.getByText(/脚本结束或超时会回收其自有进程树/)).toBeInTheDocument();
    expect(screen.getByText(/批处理（.bat\/\.cmd）不接受引号或 shell 元字符/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '返回配置' }));
    await user.click(screen.getByRole('button', { name: '预览规则' }));
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
    expect(screen.queryByText(/退出码不保证外部业务完成/)).not.toBeInTheDocument();
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
    runAutomationNowMock.mockResolvedValue({ status: 'accepted' });
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
    await waitFor(() => expect(runAutomationNowMock).toHaveBeenCalledWith('daily-agent', expect.any(String)));
  });

  it.each([
    ['script', '立即运行'],
    ['notification', '立即提醒'],
  ])('preserves manual execution for a disabled %s definition', async (executorKind, buttonLabel) => {
    const user = userEvent.setup();
    const item = { ...agentAutomation, id: `paused-${executorKind}`, executorKind };
    getAutomationsMock.mockResolvedValue([item]);
    runAutomationNowMock.mockResolvedValue({ status: 'accepted' });
    render(<AutomationPage />);

    const runButton = await screen.findByRole('button', { name: buttonLabel });
    expect(runButton).toBeEnabled();
    await user.click(runButton);
    await waitFor(() => expect(runAutomationNowMock).toHaveBeenCalledWith(item.id, expect.any(String)));
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
    getDashboardMock.mockResolvedValue({ acceptedRequests: [], items: [{ ...agentAutomation, latestRun: {
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

    expect(await screen.findByText('结果未知')).toBeInTheDocument();
    expect(screen.getByText('已暂停，不会自动重试')).toBeInTheDocument();
    expect(screen.getByText('先核对脚本的实际状态，确认本次已结束且不会重复操作后再运行。')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '核对后重新运行' })).toBeEnabled();
    expect(screen.getByRole('button', { name: '启用' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: '停止本次执行' })).not.toBeInTheDocument();
    expect(runAutomationNowMock).not.toHaveBeenCalled();
    expect(setAutomationEnabledMock).not.toHaveBeenCalled();
  });

  it('blocks duplicate run and delete while its own request is pending but still allows owned stop and future pause', async () => {
    const user = userEvent.setup();
    let finishRun!: () => void;
    runAutomationNowMock.mockReturnValueOnce(new Promise<ManualAutomationStart>(resolve => { finishRun = () => resolve({ status: 'accepted' }); }));
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

  it.each([
    scriptAutomation,
    maaAutomation,
    { ...agentAutomation, enabled: true },
    { ...scriptAutomation, executorKind: 'notification', triggerKind: 'once', lastRunAt: undefined },
  ])('retains an unconfirmed $executorKind request after empty or failed readback without replaying it', async (item) => {
    const user = userEvent.setup();
    getAutomationsMock.mockResolvedValue([item]);
    runAutomationNowMock.mockRejectedValueOnce('此脚本已有运行中回执，请等待结果');
    render(<AutomationPage />);

    const label = item.executorKind === 'agent' ? '立即执行' : item.executorKind === 'notification' ? '立即提醒' : '立即运行';
    await user.click(await screen.findByRole('button', { name: label }));
    expect(await screen.findByRole('alert')).toHaveTextContent('此脚本已有运行中回执，请等待结果');
    await waitFor(() => expect(getDashboardMock).toHaveBeenCalledTimes(2));
    const unconfirmed = screen.getByRole('button', { name: '结果未确认' });
    expect(unconfirmed).toBeDisabled();
    expect(screen.getByRole('button', { name: `删除 ${item.title}` })).toBeDisabled();
    expect(screen.getByRole('button', { name: '暂停' })).toBeEnabled();
    await user.click(unconfirmed);
    await user.click(screen.getByRole('button', { name: '刷新' }));
    expect(screen.getByRole('button', { name: '结果未确认' })).toBeDisabled();

    getDashboardMock.mockRejectedValueOnce(new Error('readback unavailable'));
    await user.click(screen.getByRole('button', { name: '刷新' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('无法加载自动化任务');
    expect(screen.getByRole('button', { name: '结果未确认' })).toBeDisabled();
    getAutomationsMock.mockResolvedValue([{ ...item, enabled: false }]);
    await user.click(screen.getByRole('button', { name: '暂停' }));
    await waitFor(() => expect(setAutomationEnabledMock).toHaveBeenCalledWith(item.id, false));
    expect(screen.getByRole('button', { name: '暂不能启用' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '结果未确认' })).toBeDisabled();
    expect(runAutomationNowMock).toHaveBeenCalledTimes(1);
    expect(deleteAutomationMock).not.toHaveBeenCalled();
  });

  it('refreshes already opened history after an unconfirmed request without using it as a completion receipt', async () => {
    const user = userEvent.setup();
    getAutomationsMock.mockResolvedValue([scriptAutomation]);
    getHistoryMock.mockResolvedValue([{ id: 'history-only', status: 'completed', summary: '仅历史读回的结果', output: '', startedAt: 2 }]);
    runAutomationNowMock.mockRejectedValueOnce('执行响应丢失');
    render(<AutomationPage />);
    await screen.findByRole('button', { name: '立即运行' });
    await user.click(screen.getByText('运行记录与输出'));
    await waitFor(() => expect(getHistoryMock).toHaveBeenCalledTimes(1));

    await user.click(screen.getByRole('button', { name: '立即运行' }));
    await waitFor(() => expect(getHistoryMock).toHaveBeenCalledTimes(2));
    expect(screen.getByRole('button', { name: '结果未确认' })).toBeDisabled();
    expect(runAutomationNowMock).toHaveBeenCalledTimes(1);
  });

  it('reads back a persisted run after a dispatch error without claiming the request was never executed', async () => {
    const user = userEvent.setup();
    const completed = { id: 'new-run', status: 'completed', summary: '已保存的脚本结果', output: '', startedAt: 2 };
    getAutomationsMock.mockResolvedValue([scriptAutomation]);
    getAutomationRunsMock.mockResolvedValueOnce([]).mockResolvedValue([completed]);
    runAutomationNowMock.mockRejectedValueOnce('连接中断');
    render(<AutomationPage />);

    await user.click(await screen.findByRole('button', { name: '立即运行' }));
    expect(await screen.findByText(completed.summary)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '结果未确认' })).toBeDisabled();
    expect(screen.getByText('运行记录（未关联本次请求）')).toBeInTheDocument();
    expect(runAutomationNowMock).toHaveBeenCalledTimes(1);
    expect(getDashboardMock).toHaveBeenCalledTimes(2);
    expect(getHistoryMock).not.toHaveBeenCalled();
  });

  it('does not attribute old or newer scheduled receipts to an unconfirmed manual request', async () => {
    const user = userEvent.setup();
    const old = { id: 'previous-run', status: 'completed', summary: '上次脚本结果', output: '', startedAt: 1 };
    getAutomationsMock.mockResolvedValue([scriptAutomation]);
    getAutomationRunsMock.mockResolvedValue([old]);
    runAutomationNowMock.mockRejectedValueOnce('请求连接断开');
    render(<AutomationPage />);
    await user.click(await screen.findByRole('button', { name: '立即运行' }));
    expect(await screen.findByRole('button', { name: '结果未确认' })).toBeDisabled();
    expect(screen.getByText('运行记录（未关联本次请求）')).toBeInTheDocument();

    getAutomationRunsMock.mockResolvedValue([{ ...old, id: 'another-old-run', startedAt: 2 }]);
    await user.click(screen.getByRole('button', { name: '刷新' }));
    expect(screen.getByRole('button', { name: '结果未确认' })).toBeDisabled();
    getAutomationRunsMock.mockResolvedValue([{ ...old, id: 'new-scheduled-run', startedAt: 3, summary: '新计划任务的结果' }]);
    await user.click(screen.getByRole('button', { name: '刷新' }));
    expect(await screen.findByText('新计划任务的结果')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '结果未确认' })).toBeDisabled();
    expect(runAutomationNowMock).toHaveBeenCalledTimes(1);
  });

  it('resolves a lost manual acknowledgement by exact receipt after returning to the page', async () => {
    const user = userEvent.setup();
    getAutomationsMock.mockResolvedValue([scriptAutomation]);
    runAutomationNowMock.mockRejectedValueOnce('请求连接断开');
    const first = render(<AutomationPage />);
    await user.click(await screen.findByRole('button', { name: '立即运行' }));
    expect(await screen.findByRole('button', { name: '结果未确认' })).toBeDisabled();
    const [[automationId, runId]] = runAutomationNowMock.mock.calls;
    first.unmount();

    render(<AutomationPage />);
    expect(await screen.findByRole('button', { name: '结果未确认' })).toBeDisabled();
    expect(getDashboardMock).toHaveBeenLastCalledWith([{ automationId, runId }]);
    const receipt = { status: 'completed', summary: '脚本执行完成', output: '', startedAt: 2 };
    getAutomationRunsMock.mockResolvedValue([
      { ...receipt, id: 'newer-scheduled-run', startedAt: 3 },
      { ...receipt, id: runId },
    ]);
    await user.click(screen.getByRole('button', { name: '刷新' }));
    await waitFor(() => expect(screen.getByRole('button', { name: '立即运行' })).toBeEnabled());
    expect(runAutomationNowMock).toHaveBeenCalledTimes(1);
  });

  it('shows a confirmed pre-dispatch rejection without locking the task', async () => {
    const user = userEvent.setup();
    getAutomationsMock.mockResolvedValue([scriptAutomation]);
    runAutomationNowMock.mockResolvedValueOnce({ status: 'rejected', error: '任务已不存在' });
    render(<AutomationPage />);
    await user.click(await screen.findByRole('button', { name: '立即运行' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('任务已不存在');
    expect(screen.getByRole('button', { name: '立即运行' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: '结果未确认' })).not.toBeInTheDocument();
  });

  it('keeps different automation cards independently busy until their own actions settle', async () => {
    let finishFirst!: () => void;
    let finishSecond!: () => void;
    const second = { ...scriptAutomation, id: 'second-script', title: '另一脚本' };
    getAutomationsMock.mockResolvedValue([scriptAutomation, second]);
    setAutomationEnabledMock
      .mockReturnValueOnce(new Promise(resolve => { finishFirst = resolve; }))
      .mockReturnValueOnce(new Promise(resolve => { finishSecond = resolve; }));
    render(<AutomationPage />);
    const firstCard = within(await screen.findByRole('article', { name: '现有脚本' }));
    const secondCard = within(screen.getByRole('article', { name: '另一脚本' }));

    fireEvent.click(firstCard.getByRole('button', { name: '暂停' }));
    expect(firstCard.getByRole('button', { name: '暂停' })).toBeDisabled();
    expect(secondCard.getByRole('button', { name: '暂停' })).toBeEnabled();
    fireEvent.click(secondCard.getByRole('button', { name: '暂停' }));
    expect(firstCard.getByRole('button', { name: '暂停' })).toBeDisabled();
    expect(secondCard.getByRole('button', { name: '暂停' })).toBeDisabled();

    await act(async () => { finishSecond(); });
    expect(secondCard.getByRole('button', { name: '暂停' })).toBeEnabled();
    expect(firstCard.getByRole('button', { name: '暂停' })).toBeDisabled();
    await act(async () => { finishFirst(); });
    expect(firstCard.getByRole('button', { name: '暂停' })).toBeEnabled();
    expect(setAutomationEnabledMock.mock.calls).toEqual([[scriptAutomation.id, false], [second.id, false]]);
  });

  it('coalesces a burst of automation updates into one trailing read of the latest receipt', async () => {
    let finishOldLoad!: (items: typeof scriptAutomation[]) => void;
    getAutomationsMock.mockReturnValueOnce(new Promise(resolve => { finishOldLoad = resolve; }))
      .mockResolvedValue([scriptAutomation]);
    getAutomationRunsMock.mockResolvedValue([{
      id: 'done', status: 'completed', summary: '脚本已退出，退出码 0', exitCode: 0, output: '', startedAt: 2,
    }]);
    render(<AutomationPage />);

    await waitFor(() => expect(getDashboardMock).toHaveBeenCalledTimes(1));
    for (let i = 0; i < 5; i += 1) fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT));
    expect(getDashboardMock).toHaveBeenCalledTimes(1);
    await act(async () => { finishOldLoad([{ ...scriptAutomation, title: '旧版本标题' }]); });
    expect(await screen.findByText('脚本已退出，退出码 0')).toBeInTheDocument();
    expect(getDashboardMock).toHaveBeenCalledTimes(2);
    expect(getWorkspaces).toHaveBeenCalledTimes(2);
    expect(screen.getByText('现有脚本')).toBeInTheDocument();
    expect(screen.queryByText('旧版本标题')).not.toBeInTheDocument();
  });

  it.each([
    ['running', '已启动，等待结果', 'MAA 启动已登记'],
    ['failed', '队列有失败', '队列结束，部分任务失败'],
    ['attention_required', '结果未知', 'MAA 队列结果未知'],
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
    expect(getHistoryMock).not.toHaveBeenCalled();
    if (status === 'running') {
      expect(screen.getByRole('button', { name: '等待结果' })).toBeDisabled();
      expect(screen.getByRole('button', { name: '删除 MAA 日常队列' })).toBeDisabled();
      expect(screen.getByRole('button', { name: '暂停' })).toBeEnabled();
      expect(screen.getByText(/不能仅凭此回执确认MAA 队列仍在运行/)).toBeInTheDocument();
    } else if (status === 'attention_required') {
      expect(screen.getByRole('button', { name: '核对后重新运行' })).toBeEnabled();
      expect(screen.getByRole('button', { name: '启用' })).toBeDisabled();
      expect(screen.getByText('后续定时')).toBeInTheDocument();
      expect(screen.getByText('已暂停，不会自动重试')).toBeInTheDocument();
      expect(screen.getByText(/先核对 MAA 的实际状态/)).toBeInTheDocument();
    } else if (status === 'failed') {
      expect(screen.getByText('下次执行')).toBeInTheDocument();
      expect(screen.queryByText('已暂停，不会自动重试')).not.toBeInTheDocument();
      expect(screen.getByText(/再到 MAA 核对失败项/)).toBeInTheDocument();
    } else if (status === 'completed') {
      expect(screen.queryByLabelText('MAA 日常队列 查看运行记录')).not.toBeInTheDocument();
      expect(screen.getByText('记录的是 MAA 队列执行结果，不代表游戏业务目标全部完成。')).toBeInTheDocument();
    }
    if (status === 'failed' || status === 'attention_required') {
      const summary = screen.getByLabelText('MAA 日常队列 查看运行记录');
      expect(within(summary).getAllByText('查看运行记录')).toHaveLength(1);
      expect(within(summary).getByText('本次记录')).toBeInTheDocument();
      fireEvent.click(summary);
      await waitFor(() => expect(getHistoryMock).toHaveBeenCalledTimes(1));
      expect(getHistoryMock).toHaveBeenCalledWith(maaAutomation.id);
      expect(screen.getByRole('button', { name: status === 'failed' ? '立即运行' : '核对后重新运行' })).not.toHaveClass('automation-run-button');
      expect(setAutomationEnabledMock).not.toHaveBeenCalled();
    }
    expect(runAutomationNowMock).not.toHaveBeenCalled();
  });
});
