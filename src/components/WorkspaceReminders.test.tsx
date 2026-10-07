import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getAutomations, type Automation } from '$lib/commands/automation';
import {
  AUTOMATIONS_UPDATED_EVENT,
  pickTodayAutomations,
  pickTodayTriggeredAutomations,
  WorkspaceReminders,
} from './WorkspaceReminders';

vi.mock('$lib/commands/automation', () => ({ getAutomations: vi.fn() }));
const getAutomationsMock = vi.mocked(getAutomations);
const now = new Date(2026, 9, 3, 15, 0, 0);
const todayAt = (hour: number) => new Date(2026, 9, 3, hour, 0, 0).getTime() / 1000;

function automation(overrides: Partial<Automation> = {}): Automation {
  return {
    id: 'reminder-1', title: '记得喝水', prompt: '', triggerKind: 'once',
    triggerValue: '2026-10-03T16:00:00', enabled: true,
    permissionSummary: '', executorKind: 'notification', workspaceId: 'personal',
    scriptArgs: [], timeoutSeconds: 300, nextRunAt: todayAt(16),
    ...overrides,
  };
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
  getAutomationsMock.mockResolvedValue([]);
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
      automation({ id: 'unscoped', workspaceId: undefined }),
      automation({ id: 'project', workspaceId: 'project-a' }),
      automation({ id: 'invalid', nextRunAt: Number.NaN }),
      automation({ id: 'infinite', nextRunAt: Infinity }),
      automation({ id: 'missing', nextRunAt: undefined }),
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
      automation({ id: 'cancelled', enabled: false, lastRunAt: undefined }),
      automation({ id: 'fired-once', enabled: false, nextRunAt: undefined, lastRunAt: todayAt(14) }),
      automation({ id: 'earlier', lastRunAt: todayAt(8) }),
      automation({ id: 'earlier', lastRunAt: todayAt(8) }),
      automation({ id: 'yesterday', lastRunAt: todayAt(14) - 86400 }),
      automation({ id: 'future', lastRunAt: todayAt(16) }),
      automation({ id: 'invalid', lastRunAt: Number.NaN }),
      automation({ id: 'project', workspaceId: 'project-a', lastRunAt: todayAt(14) }),
      automation({ id: 'unscoped', workspaceId: undefined, lastRunAt: todayAt(14) }),
    ];
    expect(pickTodayTriggeredAutomations(items, now, 'personal').map((item) => item.id)).toEqual(['fired-once', 'earlier']);
  });
});

describe('WorkspaceReminders', () => {
  it('事项回看同时呈现待执行Agent任务与已触发通知，不将触发说成任务完成', async () => {
    getAutomationsMock.mockResolvedValue([
      automation({
        id: 'daily-agent', title: '每日整理待办', executorKind: 'agent',
        triggerKind: 'schedule', triggerValue: '每天 16:00',
      }),
      automation({
        id: 'fired-notification', title: '喝水提醒', enabled: false,
        nextRunAt: undefined, lastRunAt: todayAt(14),
      }),
    ]);
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
    getAutomationsMock.mockResolvedValue([
      automation(),
      automation({ id: 'fired', title: '已触发的提醒', enabled: false, nextRunAt: undefined, lastRunAt: earlier }),
      automation({ id: 'cancelled', title: '取消的事项', enabled: false, lastRunAt: undefined }),
      automation({ id: 'other', title: '另一个空间的提醒', workspaceId: 'project' }),
    ]);
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
    getAutomationsMock.mockResolvedValue([
      ...Array.from({ length: 4 }, (_, i) => automation({ id: `pending-${i}`, title: `待提醒-${i}`, nextRunAt: todayAt(16) + 86400 * (i + 1) })),
      ...Array.from({ length: 4 }, (_, i) => automation({ id: `triggered-${i}`, title: `触发-${i}`, enabled: false, lastRunAt: earlier - i })),
    ]);
    render(<WorkspaceReminders workspaceId="personal" defaultOpen />);

    expect(await screen.findByText('待执行 4 · 今日已触发 4')).toBeVisible();
    const pendingSection = screen.getByRole('region', { name: '待执行' });
    const triggeredSection = screen.getByRole('region', { name: '今日已触发' });
    expect(within(pendingSection).getAllByRole('listitem')).toHaveLength(3);
    expect(within(triggeredSection).getAllByRole('listitem')).toHaveLength(3);
    expect(screen.queryByText('待提醒-3')).not.toBeInTheDocument();
    expect(screen.queryByText('触发-3')).not.toBeInTheDocument();
  });

  it.each([null, undefined, ''])('未确定空间%p时不请求也不显示提醒', (workspaceId) => {
    render(<WorkspaceReminders workspaceId={workspaceId} />);
    expect(getAutomationsMock).not.toHaveBeenCalled();
    expect(screen.queryByText('事项回看')).not.toBeInTheDocument();
  });

  it('空列表或初次加载失败时静默隐藏，不阻断主对话', async () => {
    getAutomationsMock.mockRejectedValue(new Error('ipc down'));
    render(<WorkspaceReminders workspaceId="personal" />);
    await waitFor(() => expect(getAutomationsMock).toHaveBeenCalledOnce());
    expect(screen.queryByText('事项回看')).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('现有调度扫描更新事件和重新聚焦会刷新，没有新增轮询', async () => {
    getAutomationsMock.mockResolvedValueOnce([automation()]).mockResolvedValueOnce([]).mockResolvedValueOnce([automation({ title: '新提醒' })]);
    const intervalSpy = vi.spyOn(window, 'setInterval');
    await act(async () => { render(<WorkspaceReminders workspaceId="personal" />); });
    expect(screen.getByText('记得喝水')).toBeInTheDocument();

    await act(async () => { fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT)); });
    expect(screen.queryByText('事项回看')).not.toBeInTheDocument();
    await act(async () => { fireEvent.focus(window); });
    expect(screen.getByText('新提醒')).toBeInTheDocument();
    expect(getAutomationsMock).toHaveBeenCalledTimes(3);
    expect(intervalSpy).not.toHaveBeenCalled();
    intervalSpy.mockRestore();
  });

  it('切换空间立即隐藏旧数据，并忽略上一空间迟到的响应', async () => {
    const oldRequest = deferred<Automation[]>();
    getAutomationsMock.mockReturnValueOnce(oldRequest.promise).mockResolvedValueOnce([
      automation({ id: 'project', title: '项目提醒', workspaceId: 'project' }),
    ]);
    const { rerender } = render(<WorkspaceReminders workspaceId="personal" />);
    rerender(<WorkspaceReminders workspaceId="project" />);
    await screen.findByText('项目提醒');

    await act(async () => { oldRequest.resolve([automation({ title: '个人旧提醒' })]); });
    expect(screen.getByText('项目提醒')).toBeInTheDocument();
    expect(screen.queryByText('个人旧提醒')).not.toBeInTheDocument();
  });

  it('已显示个人提醒切换到未确定空间时立刻清空，不作全局查询', async () => {
    getAutomationsMock.mockResolvedValue([automation()]);
    const { rerender } = render(<WorkspaceReminders workspaceId="personal" />);
    await screen.findByText('记得喝水');
    rerender(<WorkspaceReminders workspaceId={null} />);
    expect(screen.queryByText('记得喝水')).not.toBeInTheDocument();
    expect(getAutomationsMock).toHaveBeenCalledOnce();
  });

  it('同空间重叠刷新只采纳最新响应，旧请求失败不会抹掉新数据', async () => {
    const first = deferred<Automation[]>();
    const second = deferred<Automation[]>();
    getAutomationsMock.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    render(<WorkspaceReminders workspaceId="personal" />);
    fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT));

    await act(async () => { second.resolve([automation({ title: '最新提醒' })]); });
    expect(screen.getByText('最新提醒')).toBeInTheDocument();
    await act(async () => { first.reject(new Error('old IPC failed')); });
    expect(screen.getByText('最新提醒')).toBeInTheDocument();
  });

  it('卸载后移除事件监听且忽略尚未完成的请求', async () => {
    const request = deferred<Automation[]>();
    getAutomationsMock.mockReturnValueOnce(request.promise);
    const { unmount } = render(<WorkspaceReminders workspaceId="personal" />);
    unmount();
    fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT));
    fireEvent.focus(window);
    await act(async () => { request.resolve([automation()]); });
    expect(getAutomationsMock).toHaveBeenCalledOnce();
    expect(screen.queryByText('事项回看')).not.toBeInTheDocument();
  });
});
