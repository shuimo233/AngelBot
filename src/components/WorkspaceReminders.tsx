import { useEffect, useId, useRef, useState } from 'react';
import { getWorkspaceAutomationOverview, type WorkspaceAutomationStatus } from '$lib/commands/automation';
import { subscribeAutomationUpdates } from '$lib/automation-events';
import { useNavigationStore } from '$stores/navigation';
import './WorkspaceReminders.css';

const MAX_REMINDERS = 3;

function isSameLocalDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear()
    && a.getMonth() === b.getMonth()
    && a.getDate() === b.getDate();
}

function isValidTimestamp(value: number | null): value is number {
  return typeof value === 'number' && value > 0 && Number.isFinite(value)
    && Number.isFinite(new Date(value * 1000).getTime());
}

/** Missing workspace identity must never fall back to a global reminder list. */
function scopedAutomations(automations: WorkspaceAutomationStatus[], workspaceId?: string | null): WorkspaceAutomationStatus[] {
  if (!workspaceId) return [];
  const byId = new Map<string, WorkspaceAutomationStatus>();
  for (const item of automations) {
    if (item.workspaceId === workspaceId && item.id && !byId.has(item.id)) {
      byId.set(item.id, item);
    }
  }
  return [...byId.values()];
}

function pendingAutomations(automations: WorkspaceAutomationStatus[], workspaceId?: string | null): WorkspaceAutomationStatus[] {
  return scopedAutomations(automations, workspaceId)
    .filter((item) => item.enabled && isValidTimestamp(item.nextRunAt))
    .sort((a, b) => (a.nextRunAt! - b.nextRunAt!) || a.id.localeCompare(b.id));
}

/** Enabled schedules due today, including overdue items not yet triggered. */
export function pickTodayAutomations(
  automations: WorkspaceAutomationStatus[],
  now: Date = new Date(),
  workspaceId?: string | null,
): WorkspaceAutomationStatus[] {
  return pendingAutomations(automations, workspaceId)
    .filter((item) => isSameLocalDay(new Date(item.nextRunAt! * 1000), now))
    .slice(0, MAX_REMINDERS);
}

/** Each automation contributes only its latest recorded trigger, not delivery proof. */
export function pickTodayTriggeredAutomations(
  automations: WorkspaceAutomationStatus[],
  now: Date = new Date(),
  workspaceId?: string | null,
): WorkspaceAutomationStatus[] {
  return scopedAutomations(automations, workspaceId)
    .filter((item) => isValidTimestamp(item.lastRunAt)
      && item.lastRunAt * 1000 <= now.getTime()
      && isSameLocalDay(new Date(item.lastRunAt * 1000), now))
    .sort((a, b) => (b.lastRunAt! - a.lastRunAt!) || a.id.localeCompare(b.id));
}

function executorLabel(item: WorkspaceAutomationStatus): string {
  if (item.executorKind === 'notification') return '本地提醒';
  if (item.executorKind === 'script') return '脚本任务';
  if (item.executorKind === 'maa') return 'MAA 队列';
  return '自动任务';
}

function needsAttention(item: WorkspaceAutomationStatus): boolean {
  return !!item.latestRun && ['attention_required', 'needs_attention', 'failed', 'timed_out', 'interrupted'].includes(item.latestRun.status);
}

function resultLabel(item: WorkspaceAutomationStatus): string {
  const run = item.latestRun;
  if (!run) return '';
  if (run.status === 'attention_required' || run.status === 'needs_attention') return '需要处理';
  if (run.status === 'running') return run.stopControl === 'requested' ? '正在停止，等待清理结果' : '已登记开始，等待结果';
  if (run.status === 'queued') return '已排队，等待执行';
  if (run.status === 'cancelled') return item.executorKind === 'script' || item.executorKind === 'maa' ? '本次已停止' : '本次已取消';
  if (run.status === 'completed') {
    if (item.executorKind === 'maa') return 'MAA 队列已结束';
    if (item.executorKind === 'script') return '脚本已退出';
    if (item.executorKind === 'notification') return '提醒已触发';
    return '主 Agent 已结束';
  }
  if (run.status === 'failed') return '执行有失败';
  if (run.status === 'timed_out') return '本次已超时';
  if (run.status === 'interrupted') return '本次已中断';
  if (run.status === 'skipped') return item.executorKind === 'notification' ? '本次未发送' : '本次未启动';
  return '查看自动化记录';
}

function resultPriority(item: WorkspaceAutomationStatus): number {
  return needsAttention(item) ? 0 : item.latestRun?.status === 'running' ? 1 : 2;
}

function resultTimestamp(item: WorkspaceAutomationStatus): number {
  return Math.max(item.latestRun?.startedAt ?? 0, item.lastSkippedAt ?? 0);
}

function displayTime(timestamp: number, now: Date): string {
  const date = new Date(timestamp * 1000);
  const time = date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  return isSameLocalDay(date, now)
    ? `今天 ${time}`
    : `${date.toLocaleDateString([], { month: 'numeric', day: 'numeric' })} ${time}`;
}

interface ReminderSnapshot {
  workspaceId: string;
  automations: WorkspaceAutomationStatus[];
  truncated: boolean;
}

export interface WorkspaceRemindersProps {
  workspaceId: string | null | undefined;
  defaultOpen?: boolean;
}

/** Read-only automation recall for the one Main conversation; never schedules work. */
export function WorkspaceReminders({ workspaceId, defaultOpen = false }: WorkspaceRemindersProps) {
  const [snapshot, setSnapshot] = useState<ReminderSnapshot | null>(null);
  const [failedWorkspaceId, setFailedWorkspaceId] = useState<string | null>(null);
  const refreshRef = useRef<() => void>(() => {});
  const setPage = useNavigationStore((state) => state.setPage);
  const openAutomation = useNavigationStore((state) => state.openAutomation);
  const headingId = useId();

  useEffect(() => {
    let disposed = false;
    let latestRequest = 0;
    setSnapshot(null);
    setFailedWorkspaceId(null);
    refreshRef.current = () => {};
    if (!workspaceId) return;

    const refresh = async () => {
      const request = ++latestRequest;
      try {
        const overview = await getWorkspaceAutomationOverview(workspaceId);
        if (!disposed && request === latestRequest) {
          setSnapshot({ workspaceId, automations: overview.items, truncated: overview.truncated });
          setFailedWorkspaceId(null);
        }
      } catch {
        // Keep known results visible while a refresh fails; chat remains usable.
        if (!disposed && request === latestRequest) setFailedWorkspaceId(workspaceId);
      }
    };

    refreshRef.current = () => { void refresh(); };
    void refresh();
    const unsubscribe = subscribeAutomationUpdates(() => { void refresh(); });
    window.addEventListener('focus', refresh);
    return () => {
      disposed = true;
      refreshRef.current = () => {};
      unsubscribe();
      window.removeEventListener('focus', refresh);
    };
  }, [workspaceId]);

  // Hide the previous space synchronously, before the next effect runs.
  if (!workspaceId) return null;
  const currentSnapshot = snapshot?.workspaceId === workspaceId ? snapshot : null;
  const refreshFailed = failedWorkspaceId === workspaceId;
  if (!currentSnapshot && !refreshFailed) return null;
  const now = new Date();
  const items = scopedAutomations(currentSnapshot?.automations ?? [], workspaceId);
  const pending = pendingAutomations(items, workspaceId);
  const triggered = pickTodayTriggeredAutomations(items, now, workspaceId);
  const results = items.filter((item) => item.latestRun || isValidTimestamp(item.lastSkippedAt))
    .sort((a, b) => resultPriority(a) - resultPriority(b)
      || resultTimestamp(b) - resultTimestamp(a) || a.id.localeCompare(b.id));
  const attentionCount = results.filter(needsAttention).length;
  const runningCount = results.filter(item => item.latestRun?.status === 'running').length;
  const endedCount = results.filter(item => item.latestRun && ['completed', 'cancelled'].includes(item.latestRun.status)).length;
  if (!pending.length && !triggered.length && !results.length && !refreshFailed && !currentSnapshot?.truncated) return null;

  return (
    <details key={workspaceId} className="workspace-reminders" open={defaultOpen}>
      <summary className="workspace-reminders-summary">
        <span id={headingId} className="workspace-reminders-title">事项回看</span>
        <span className="workspace-reminders-counts">
          {currentSnapshot ? <>待执行 {pending.length} · 今日已触发 {triggered.length}
            {attentionCount > 0 && <> · 需要处理 {attentionCount}</>}
            {runningCount > 0 && <> · 等待结果 {runningCount}</>}
            {endedCount > 0 && <> · 已结束 {endedCount}</>}
            {refreshFailed && <> · 暂未更新</>}
          </> : '暂时无法读取'}
        </span>
      </summary>
      <div className="workspace-reminders-content" aria-labelledby={headingId}>
        {refreshFailed && <p className="workspace-reminders-note" role="status">
          暂时无法刷新事项{currentSnapshot ? '，以下保留上次读取的结果' : ''}；聊天仍可继续。
        </p>}
        {results.length > 0 && (
          <section className="workspace-reminders-section" aria-label="运行结果">
            <h3 className="messages-empty-section-title">运行结果</h3>
            <ul className="messages-empty-list workspace-reminders-list workspace-reminders-results">
              {results.slice(0, MAX_REMINDERS).map((item) => (
                <li key={item.id}>
                  <span className="messages-empty-item-name" title={item.title}>{item.title}</span>
                  <div className="workspace-reminders-result-detail">
                    {item.latestRun && <>
                      <span className={needsAttention(item) ? 'workspace-reminders-attention' : 'messages-empty-item-meta'}>
                        {resultLabel(item)}{item.executorKind === 'script' && ['completed', 'failed'].includes(item.latestRun.status) && item.latestRun.exitCode != null ? ` · 退出码 ${item.latestRun.exitCode}` : ''}
                        {isValidTimestamp(item.latestRun.startedAt) && ` · ${displayTime(item.latestRun.startedAt, now)}`}
                      </span>
                      {item.latestRun.summary && <p className="workspace-reminders-note">{item.latestRun.summary}</p>}
                    </>}
                    {isValidTimestamp(item.lastSkippedAt) && <p className="workspace-reminders-note">
                      本次未启动 · {displayTime(item.lastSkippedAt, now)}；查看自动化记录了解原因。
                    </p>}
                  </div>
                  <button type="button" onClick={() => openAutomation({ automationId: item.id, workspaceId: item.workspaceId })} aria-label={`查看 ${item.title} 的运行记录`}>查看记录</button>
                </li>
              ))}
            </ul>
            {results.length > MAX_REMINDERS && <p className="workspace-reminders-note">仅展示最近 {MAX_REMINDERS} 项结果，优先显示需要处理和等待结果的事项。</p>}
          </section>
        )}
        {pending.length > 0 && (
          <section className="workspace-reminders-section" aria-label="待执行">
            <h3 className="messages-empty-section-title">待执行</h3>
            <ul className="messages-empty-list workspace-reminders-list">
              {pending.slice(0, MAX_REMINDERS).map((item) => (
                <li key={item.id}>
                  <span className="messages-empty-item-name" title={item.title}>{item.title}</span>
                  <span className="messages-empty-item-meta">
                    {executorLabel(item)} · {displayTime(item.nextRunAt!, now)}
                  </span>
                </li>
              ))}
            </ul>
            {pending.length > MAX_REMINDERS && <p className="workspace-reminders-note">仅显示最近 {MAX_REMINDERS} 条待执行事项</p>}
          </section>
        )}
        {triggered.length > 0 && (
          <section className="workspace-reminders-section" aria-label="今日已触发">
            <h3 className="messages-empty-section-title">今日已触发</h3>
            <ul className="messages-empty-list workspace-reminders-list">
              {triggered.slice(0, MAX_REMINDERS).map((item) => (
                <li key={item.id}>
                  <span className="messages-empty-item-name" title={item.title}>{item.title}</span>
                  <span className="messages-empty-item-meta">
                    已触发 · {displayTime(item.lastRunAt!, now)}
                  </span>
                </li>
              ))}
            </ul>
            <p className="workspace-reminders-note">
              每项显示最近一次触发{triggered.length > MAX_REMINDERS ? `，仅展示最近 ${MAX_REMINDERS} 项` : ''}；不代表任务已完成或系统通知已送达。
            </p>
          </section>
        )}
        {currentSnapshot?.truncated && <p className="workspace-reminders-note">当前仅显示部分事项；完整记录请查看自动化页。</p>}
        <div className="workspace-reminders-actions">
          <button type="button" onClick={() => setPage('tasks')}>查看自动化页</button>
          <button type="button" onClick={() => refreshRef.current()}>刷新</button>
        </div>
      </div>
    </details>
  );
}
