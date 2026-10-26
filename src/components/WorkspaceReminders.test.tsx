import { act, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getWorkspaceAutomationOverview, type WorkspaceAutomationOverview, type WorkspaceAutomationStatus } from '$lib/commands/automation';
import { AUTOMATIONS_UPDATED_EVENT } from '$lib/automation-events';
import { useNavigationStore } from '$stores/navigation';
import {
  pickTodayAutomations,
  pickTodayTriggeredAutomations,
  WorkspaceReminders,
} from './WorkspaceReminders';

vi.mock('$lib/commands/automation', () => ({ getWorkspaceAutomationOverview: vi.fn() }));
const getOverviewMock = vi.mocked(getWorkspaceAutomationOverview);
const now = new Date(2026, 9, 3, 15, 0, 0);
const todayAt = (hour: number) => new Date(2026, 9, 3, hour, 0, 0).getTime() / 1000;

function automation(overrides: Partial<WorkspaceAutomationStatus> = {}): WorkspaceAutomationStatus {
  return {
    id: 'reminder-1', title: '记得喝水', triggerKind: 'once',
    triggerValue: '2026-10-03T16:00:00', enabled: true,
    executorKind: 'notification', workspaceId: 'personal', nextRunAt: todayAt(16),
    lastRunAt: null, latestRun: null, lastSkippedAt: null, maaConfig: null,
    ...overrides,
  };
}

function overview(items: WorkspaceAutomationStatus[] = [], truncated = false): WorkspaceAutomationOverview {
  return { items, truncated };
}

function run(status: string, startedAt = todayAt(14), summary = ''): NonNullable<WorkspaceAutomationStatus['latestRun']> {
  return { id: `run-${status}`, status, summary, exitCode: null, startedAt };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((onResolve, onReject) => { resolve = onResolve; reject = onReject; });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(now);
  vi.clearAllMocks();
  getOverviewMock.mockResolvedValue(overview());
  useNavigationStore.setState({ currentPage: 'chat', automationTarget: null });
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('workspace reminder selection', () => {
  it('未确定空间时绝不返回全局提醒', () => {
    const items = [automation()];
    expect(pickTodayAutomations(items, now)).toEqual([]);
    expect(pickTodayAutomations(items, now, null)).toEqual([]);
    expect(pickTodayTriggeredAutomations([automation({ lastRunAt: todayAt(14) })], now)).toEqual([]);
  });

  it('今日待提醒精确隔离空间，去重、过滤非法时间并限为三条', () => {
    const items = [
      automation({ id: 'late', nextRunAt: todayAt(19) }),
      automation({ id: 'early', nextRunAt: todayAt(16) }),
      automation({ id: 'early', nextRunAt: todayAt(16) }),
      automation({ id: 'middle', nextRunAt: todayAt(17) }),
      automation({ id: 'extra', nextRunAt: todayAt(20) }),
      automation({ id: 'paused', enabled: false }),
      automation({ id: 'unscoped', workspaceId: '' }),
      automation({ id: 'project', workspaceId: 'project-a' }),
      automation({ id: 'invalid', nextRunAt: Number.NaN }),
      automation({ id: 'infinite', nextRunAt: Infinity }),
      automation({ id: 'missing', nextRunAt: null }),
      automation({ id: 'tomorrow', nextRunAt: todayAt(16) + 86400 }),
    ];
    expect(pickTodayAutomations(items, now, 'personal').map((item) => item.id)).toEqual(['early', 'middle', 'late']);
  });

  it('同一时间用稳定ID排序，与接口输入顺序无关', () => {
    const items = [automation({ id: 'b' }), automation({ id: 'a' })];
    expect(pickTodayAutomations(items, now, 'personal').map((item) => item.id)).toEqual(['a', 'b']);
    expect(pickTodayAutomations([...items].reverse(), now, 'personal').map((item) => item.id)).toEqual(['a', 'b']);
  });

  it('只有今日真实lastRunAt进入记录，取消不是完成且停用的一次提醒仍能回看', () => {
    const items = [
      automation({ id: 'cancelled', enabled: false, lastRunAt: null }),
      automation({ id: 'fired-once', enabled: false, nextRunAt: null, lastRunAt: todayAt(14) }),
      automation({ id: 'earlier', lastRunAt: todayAt(8) }),
      automation({ id: 'earlier', lastRunAt: todayAt(8) }),
      automation({ id: 'yesterday', lastRunAt: todayAt(14) - 86400 }),
      automation({ id: 'future', lastRunAt: todayAt(16) }),
      automation({ id: 'invalid', lastRunAt: Number.NaN }),
      automation({ id: 'project', workspaceId: 'project-a', lastRunAt: todayAt(14) }),
      automation({ id: 'unscoped', workspaceId: '', lastRunAt: todayAt(14) }),
    ];
    expect(pickTodayTriggeredAutomations(items, now, 'personal').map((item) => item.id)).toEqual(['fired-once', 'earlier']);
  });
});

describe('WorkspaceReminders', () => {
  it('通知关闭时保留未发送回执，不称为完成或需要重试', async () => {
    getOverviewMock.mockResolvedValue(overview([
      automation({ enabled: false, nextRunAt: null, lastRunAt: todayAt(14),
        latestRun: run('skipped', todayAt(14), '通知已关闭，本次提醒未发送；不会自动补发') }),
    ]));
    render(<WorkspaceReminders workspaceId="personal" defaultOpen />);
    const results = await screen.findByRole('region', { name: '运行结果' });
    expect(within(results).getByText(/本次未发送 ·/)).toBeVisible();
    expect(within(results).getByText('通知已关闭，本次提醒未发送；不会自动补发')).toBeVisible();
    expect(screen.queryByText('已完成')).not.toBeInTheDocument();
    expect(screen.queryByText(/需要处理/)).not.toBeInTheDocument();
  });

  it('事项回看同时呈现待执行Agent任务与已触发通知，不将触发说成任务完成', async () => {
    getOverviewMock.mockResolvedValue(overview([
      automation({
        id: 'daily-agent', title: '每日整理待办', executorKind: 'agent',
        triggerKind: 'schedule', triggerValue: '每天 16:00',
      }),
      automation({
        id: 'fired-notification', title: '喝水提醒', enabled: false,
        nextRunAt: null, lastRunAt: todayAt(14),
      }),
    ]));
    render(<WorkspaceReminders workspaceId="personal" defaultOpen />);

    expect(await screen.findByText('事项回看')).toBeVisible();
    expect(screen.getByText('待执行 1 · 今日已触发 1')).toBeVisible();
    const pendingSection = screen.getByRole('region', { name: '待执行' });
    const triggeredSection = screen.getByRole('region', { name: '今日已触发' });
    expect(within(pendingSection).getByText('每日整理待办')).toBeVisible();
    expect(within(pendingSection).getByText(/自动任务/)).toBeVisible();
    expect(within(pendingSection).queryByText('喝水提醒')).not.toBeInTheDocument();
    expect(within(triggeredSection).getByText('喝水提醒')).toBeVisible();
    expect(within(triggeredSection).queryByText('每日整理待办')).not.toBeInTheDocument();
    expect(screen.getByText(/不代表任务已完成或系统通知已送达/)).toBeVisible();
    expect(screen.queryByText('待提醒')).not.toBeInTheDocument();
    expect(screen.queryByText('已完成')).not.toBeInTheDocument();
  });

  it('默认折叠，展开后分别显示待执行和今日已触发，不宣称已送达', async () => {
    const user = userEvent.setup();
    const earlier = Math.floor(Date.now() / 1000) - 1;
    getOverviewMock.mockResolvedValue(overview([
      automation(),
      automation({ id: 'fired', title: '已触发的提醒', enabled: false, nextRunAt: null, lastRunAt: earlier }),
      automation({ id: 'cancelled', title: '取消的事项', enabled: false, lastRunAt: null }),
      automation({ id: 'other', title: '另一个空间的提醒', workspaceId: 'project' }),
    ]));
    const { container } = render(<WorkspaceReminders workspaceId="personal" />);

    const summary = await screen.findByText('事项回看');
    expect(container.querySelector('details')).not.toHaveAttribute('open');
    expect(screen.getByText('待执行 1 · 今日已触发 1')).toBeInTheDocument();
    expect(screen.getByText('记得喝水')).not.toBeVisible();
    await user.click(summary);
    expect(screen.getByText('记得喝水')).toBeVisible();
    expect(screen.getByText('已触发的提醒')).toBeVisible();
    expect(screen.getByText(/不代表任务已完成或系统通知已送达/)).toBeVisible();
    expect(screen.queryByText('取消的事项')).not.toBeInTheDocument();
    expect(screen.queryByText('另一个空间的提醒')).not.toBeInTheDocument();
    expect(screen.queryByText('已完成')).not.toBeInTheDocument();
  });

  it('保留未来待执行事项，列表各限三条并显示完整计数', async () => {
    const earlier = Math.floor(Date.now() / 1000) - 1;
    getOverviewMock.mockResolvedValue(overview([
      ...Array.from({ length: 4 }, (_, i) => automation({ id: `pending-${i}`, title: `待提醒-${i}`, nextRunAt: todayAt(16) + 86400 * (i + 1) })),
      ...Array.from({ length: 4 }, (_, i) => automation({ id: `triggered-${i}`, title: `触发-${i}`, enabled: false, lastRunAt: earlier - i })),
    ]));
    render(<WorkspaceReminders workspaceId="personal" defaultOpen />);

    expect(await screen.findByText('待执行 4 · 今日已触发 4')).toBeVisible();
    const pendingSection = screen.getByRole('region', { name: '待执行' });
    const triggeredSection = screen.getByRole('region', { name: '今日已触发' });
    expect(within(pendingSection).getAllByRole('listitem')).toHaveLength(3);
    expect(within(triggeredSection).getAllByRole('listitem')).toHaveLength(3);
    expect(screen.queryByText('待提醒-3')).not.toBeInTheDocument();
    expect(screen.queryByText('触发-3')).not.toBeInTheDocument();
  });

  it.each([
    ['completed', 'MAA 队列已结束', '脚本已退出', '主 Agent 已结束'],
    ['cancelled', '本次已停止', '本次已停止', '本次已取消'],
  ])('真实 %s 结果保留自身日期，今日未启动不会变成今日成功，并可进入自动化页', async (status, maaLabel, scriptLabel, agentLabel) => {
    const yesterday = todayAt(18) - 86400;
    getOverviewMock.mockResolvedValue(overview([
      automation({ id: 'maa', title: 'MAA 日常队列', executorKind: 'maa', enabled: false, nextRunAt: null,
        latestRun: run(status, yesterday, `${maaLabel}；不代表游戏业务目标全部完成。`), lastSkippedAt: todayAt(14), lastRunAt: todayAt(14) }),
      automation({ id: 'script', title: '本地脚本', executorKind: 'script', enabled: false, nextRunAt: null,
        latestRun: { ...run(status, todayAt(14), '回执仅表示本次脚本结果，不代表外部应用的业务目标完成。'), exitCode: 0 } }),
      automation({ id: 'agent', title: '整理事项', executorKind: 'agent', enabled: false, nextRunAt: null,
        latestRun: run(status, todayAt(14), '具体结果见归属工作区对话。') }),
    ]));
    render(<WorkspaceReminders workspaceId="personal" defaultOpen />);

    const results = await screen.findByRole('region', { name: '运行结果' });
    const maaRow = within(results).getByText('MAA 日常队列').closest('li')!;
    const resultDate = new Date(yesterday * 1000).toLocaleDateString([], { month: 'numeric', day: 'numeric' });
    expect(within(maaRow).getByText(new RegExp(`${maaLabel} ·`))).toHaveTextContent(resultDate);
    expect(within(maaRow).getByText(/本次未启动 · 今天/)).toBeVisible();
    expect(within(maaRow).getByText(`${maaLabel}；不代表游戏业务目标全部完成。`)).toBeVisible();
    if (status === 'completed') expect(within(results).getByText(new RegExp(`${scriptLabel} · 退出码 0`))).toBeVisible();
    else expect(within(results).queryByText(/退出码/)).not.toBeInTheDocument();
    expect(within(results).getByText(/不代表外部应用的业务目标完成/)).toBeVisible();
    expect(within(results).getByText(new RegExp(`${agentLabel} ·`))).toBeVisible();
    await userEvent.setup().click(screen.getByRole('button', { name: '查看自动化页' }));
    expect(useNavigationStore.getState().currentPage).toBe('tasks');
  });

  it('停止请求只显示等待清理，不提前显示终态；更新后沿用结束回执', async () => {
    const item = automation({ executorKind: 'script', enabled: false, nextRunAt: null,
      latestRun: { ...run('running', todayAt(14)), stopControl: 'requested' } });
    getOverviewMock.mockResolvedValueOnce(overview([item])).mockResolvedValueOnce(overview([
      { ...item, latestRun: run('cancelled', todayAt(14)) },
    ]));
    render(<WorkspaceReminders workspaceId="personal" defaultOpen />);
    expect(await screen.findByText(/正在停止，等待清理结果/)).toBeVisible();
    expect(screen.queryByText(/本次已停止/)).not.toBeInTheDocument();
    await act(async () => { fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT)); });
    expect(await screen.findByText(/本次已停止/)).toBeVisible();
    expect(screen.queryByText(/正在停止，等待清理结果/)).not.toBeInTheDocument();
  });

  it('队列失败和脚本非零退出使用脱敏失败摘要，单独跳过不推断原因', async () => {
    getOverviewMock.mockResolvedValue(overview([
      automation({ id: 'maa', title: '队列部分失败', executorKind: 'maa', enabled: false, nextRunAt: null,
        latestRun: run('failed', todayAt(14), 'MAA 执行存在失败；部分操作可能已生效，请核对实际结果。') }),
      automation({ id: 'script', title: '脚本失败', executorKind: 'script', enabled: false, nextRunAt: null,
        latestRun: { ...run('failed', todayAt(13), '本次执行失败；请核对实际结果，查询不会自动重试。'), exitCode: 2 } }),
      automation({ id: 'skipped', title: '本次未运行的队列', executorKind: 'maa', enabled: false, nextRunAt: null,
        lastSkippedAt: todayAt(12) }),
    ]));
    render(<WorkspaceReminders workspaceId="personal" defaultOpen />);

    const results = await screen.findByRole('region', { name: '运行结果' });
    expect(within(results).getByText(/MAA 执行存在失败；部分操作可能已生效/)).toBeVisible();
    expect(within(results).getByText(/执行有失败 · 退出码 2/)).toBeVisible();
    expect(within(results).getByText(/本次未启动 · 今天/)).toHaveTextContent('查看自动化记录了解原因');
    expect(within(results).queryByText(/MAA 队列已结束/)).not.toBeInTheDocument();
    expect(within(results).queryByText(/重叠/)).not.toBeInTheDocument();
  });

  it('跨日待处理和开始记录优先且有界，刷新失败保留已知状态，恢复后移除陈旧提示', async () => {
    const yesterday = todayAt(14) - 86400;
    const resolved = automation({ id: 'resolved', title: '已有明确结果', executorKind: 'maa', enabled: false,
      nextRunAt: null, latestRun: run('completed') });
    getOverviewMock.mockResolvedValueOnce(overview([
      automation({ id: 'unknown', title: '结果未知的脚本', executorKind: 'script', enabled: false, nextRunAt: null,
        lastRunAt: yesterday, latestRun: run('attention_required', yesterday, '本次结果未知，不可自动重放；请先核对实际状态后明确处理。'), lastSkippedAt: todayAt(14) }),
      automation({ id: 'attention', title: '需要核对的 Agent', executorKind: 'agent', enabled: false, nextRunAt: null,
        latestRun: run('needs_attention', yesterday - 1) }),
      automation({ id: 'running', title: '已登记的 MAA', executorKind: 'maa', enabled: false, nextRunAt: null,
        latestRun: run('running', yesterday, '本次执行已登记，尚无结束结果；此回执不能证明进程仍然存活。') }),
      resolved,
      automation({ id: 'private', title: '另一空间的结果', workspaceId: 'project', enabled: false, nextRunAt: null,
        latestRun: run('attention_required') }),
    ], true)).mockRejectedValueOnce(new Error('ipc down')).mockResolvedValueOnce(overview([resolved]));
    render(<WorkspaceReminders workspaceId="personal" defaultOpen />);

    const results = await screen.findByRole('region', { name: '运行结果' });
    expect(screen.getByText('待执行 0 · 今日已触发 0 · 需要处理 2 · 等待结果 1 · 已结束 1')).toBeVisible();
    expect(within(results).getAllByRole('listitem')).toHaveLength(3);
    expect(within(results).getByText(/本次结果未知，不可自动重放/)).toBeVisible();
    expect(within(results).getByText(/已登记开始，等待结果/)).toBeVisible();
    expect(within(results).getByText(/此回执不能证明进程仍然存活/)).toBeVisible();
    expect(screen.queryByText('已有明确结果')).not.toBeInTheDocument();
    expect(screen.queryByText('另一空间的结果')).not.toBeInTheDocument();
    expect(screen.getByText(/当前仅显示部分事项/)).toBeVisible();
    await act(async () => { fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT)); });
    expect(await screen.findByRole('status')).toHaveTextContent('保留上次读取的结果');
    expect(screen.getByText('结果未知的脚本')).toBeVisible();
    expect(screen.getByText(/需要处理 2 · 等待结果 1 · 已结束 1 · 暂未更新/)).toBeVisible();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: '刷新' })); });
    expect(screen.getByText('已有明确结果')).toBeVisible();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(screen.queryByText('结果未知的脚本')).not.toBeInTheDocument();
  });

  it.each([null, undefined, ''])('未确定空间%p时不请求也不显示提醒', (workspaceId) => {
    render(<WorkspaceReminders workspaceId={workspaceId} />);
    expect(getOverviewMock).not.toHaveBeenCalled();
    expect(screen.queryByText('事项回看')).not.toBeInTheDocument();
  });

  it('初次加载失败提供非阻断错误和刷新，空列表仍隐藏', async () => {
    getOverviewMock.mockRejectedValueOnce(new Error('ipc down'));
    render(<WorkspaceReminders workspaceId="personal" defaultOpen />);
    expect(await screen.findByText('暂时无法读取')).toBeVisible();
    expect(screen.getByRole('status')).toHaveTextContent('聊天仍可继续');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: '刷新' })); });
    expect(screen.queryByText('事项回看')).not.toBeInTheDocument();
  });

  it('现有调度扫描更新事件和重新聚焦会刷新，没有新增轮询', async () => {
    getOverviewMock.mockResolvedValueOnce(overview([automation()])).mockResolvedValueOnce(overview()).mockResolvedValueOnce(overview([automation({ title: '新提醒' })]));
    const intervalSpy = vi.spyOn(window, 'setInterval');
    await act(async () => { render(<WorkspaceReminders workspaceId="personal" />); });
    expect(screen.getByText('记得喝水')).toBeInTheDocument();

    await act(async () => { fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT)); });
    expect(screen.queryByText('事项回看')).not.toBeInTheDocument();
    await act(async () => { fireEvent.focus(window); });
    expect(screen.getByText('新提醒')).toBeInTheDocument();
    expect(getOverviewMock).toHaveBeenCalledTimes(3);
    expect(intervalSpy).not.toHaveBeenCalled();
    intervalSpy.mockRestore();
  });

  it('切换空间立即隐藏旧数据，并忽略上一空间迟到的响应', async () => {
    const oldRequest = deferred<WorkspaceAutomationOverview>();
    getOverviewMock.mockReturnValueOnce(oldRequest.promise).mockResolvedValueOnce(overview([
      automation({ id: 'project', title: '项目提醒', workspaceId: 'project' }),
    ]));
    const { rerender } = render(<WorkspaceReminders workspaceId="personal" />);
    rerender(<WorkspaceReminders workspaceId="project" />);
    await screen.findByText('项目提醒');

    await act(async () => { oldRequest.resolve(overview([automation({ title: '个人旧提醒' })])); });
    expect(screen.getByText('项目提醒')).toBeInTheDocument();
    expect(screen.queryByText('个人旧提醒')).not.toBeInTheDocument();
    expect(getOverviewMock.mock.calls).toEqual([['personal'], ['project']]);
  });

  it('已显示个人提醒切换到未确定空间时立刻清空，不作全局查询', async () => {
    getOverviewMock.mockResolvedValue(overview([automation()]));
    const { rerender } = render(<WorkspaceReminders workspaceId="personal" />);
    await screen.findByText('记得喝水');
    rerender(<WorkspaceReminders workspaceId={null} />);
    expect(screen.queryByText('记得喝水')).not.toBeInTheDocument();
    expect(getOverviewMock).toHaveBeenCalledOnce();
  });

  it('同空间重叠刷新只采纳最新响应，旧请求失败不会抹掉新数据', async () => {
    const first = deferred<WorkspaceAutomationOverview>();
    const second = deferred<WorkspaceAutomationOverview>();
    getOverviewMock.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    render(<WorkspaceReminders workspaceId="personal" />);
    fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT));

    await act(async () => { second.resolve(overview([automation({ title: '最新提醒' })])); });
    expect(screen.getByText('最新提醒')).toBeInTheDocument();
    await act(async () => { first.reject(new Error('old IPC failed')); });
    expect(screen.getByText('最新提醒')).toBeInTheDocument();
  });

  it('opens the exact result in the dashboard without changing the current workspace or invoking work', async () => {
    getOverviewMock.mockResolvedValue(overview([automation({ latestRun: run('completed', todayAt(14), '结果摘要') })]));
    render(<WorkspaceReminders workspaceId="personal" />);
    const link = await screen.findByRole('button', { name: '查看 记得喝水 的运行记录' });
    fireEvent.click(link);
    expect(useNavigationStore.getState().currentPage).toBe('tasks');
    expect(useNavigationStore.getState().automationTarget).toEqual({ automationId: 'reminder-1', workspaceId: 'personal' });
    useNavigationStore.getState().setPage('chat');
    expect(useNavigationStore.getState().automationTarget).toBeNull();
  });

  it('卸载后移除事件监听且忽略尚未完成的请求', async () => {
    const request = deferred<WorkspaceAutomationOverview>();
    getOverviewMock.mockReturnValueOnce(request.promise);
    const { unmount } = render(<WorkspaceReminders workspaceId="personal" />);
    unmount();
    fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT));
    fireEvent.focus(window);
    await act(async () => { request.resolve(overview([automation()])); });
    expect(getOverviewMock).toHaveBeenCalledOnce();
    expect(screen.queryByText('事项回看')).not.toBeInTheDocument();
  });
});
