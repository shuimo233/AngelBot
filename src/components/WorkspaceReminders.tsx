import { useEffect, useId, useState } from 'react';
import { getAutomations, type Automation } from '$lib/commands/automation';
import './WorkspaceReminders.css';

/** Existing desktop scheduler and reminder mutations can announce fresh data. */
export const AUTOMATIONS_UPDATED_EVENT = 'angelbot:automations-updated';
const MAX_REMINDERS = 3;

function isSameLocalDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear()
    && a.getMonth() === b.getMonth()
    && a.getDate() === b.getDate();
}

function isValidTimestamp(value: number | undefined): value is number {
  return typeof value === 'number' && value > 0 && Number.isFinite(value)
    && Number.isFinite(new Date(value * 1000).getTime());
}

/** Missing workspace identity must never fall back to a global reminder list. */
function scopedAutomations(automations: Automation[], workspaceId?: string | null): Automation[] {
  if (!workspaceId) return [];
  const byId = new Map<string, Automation>();
  for (const item of automations) {
    if (item.workspaceId === workspaceId && item.id && !byId.has(item.id)) {
      byId.set(item.id, item);
    }
  }
  return [...byId.values()];
}

function pendingAutomations(automations: Automation[], workspaceId?: string | null): Automation[] {
  return scopedAutomations(automations, workspaceId)
    .filter((item) => item.enabled && isValidTimestamp(item.nextRunAt))
    .sort((a, b) => (a.nextRunAt! - b.nextRunAt!) || a.id.localeCompare(b.id));
}

/** Enabled schedules due today, including overdue items not yet triggered. */
export function pickTodayAutomations(
  automations: Automation[],
  now: Date = new Date(),
  workspaceId?: string | null,
): Automation[] {
  return pendingAutomations(automations, workspaceId)
    .filter((item) => isSameLocalDay(new Date(item.nextRunAt! * 1000), now))
    .slice(0, MAX_REMINDERS);
}

/** Each automation contributes only its latest recorded trigger, not delivery proof. */
export function pickTodayTriggeredAutomations(
  automations: Automation[],
  now: Date = new Date(),
  workspaceId?: string | null,
): Automation[] {
  return scopedAutomations(automations, workspaceId)
    .filter((item) => isValidTimestamp(item.lastRunAt)
      && item.lastRunAt * 1000 <= now.getTime()
      && isSameLocalDay(new Date(item.lastRunAt * 1000), now))
    .sort((a, b) => (b.lastRunAt! - a.lastRunAt!) || a.id.localeCompare(b.id));
}

function executorLabel(item: Automation): string {
  if (item.executorKind === 'notification') return '本地提醒';
  if (item.executorKind === 'script') return '脚本任务';
  return '自动任务';
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
  automations: Automation[];
}

export interface WorkspaceRemindersProps {
  workspaceId: string | null | undefined;
  defaultOpen?: boolean;
}

/** Read-only automation recall for the one Main conversation; never schedules work. */
export function WorkspaceReminders({ workspaceId, defaultOpen = false }: WorkspaceRemindersProps) {
  const [snapshot, setSnapshot] = useState<ReminderSnapshot | null>(null);
  const headingId = useId();

  useEffect(() => {
    let disposed = false;
    let latestRequest = 0;
    setSnapshot(null);
    if (!workspaceId) return;

    const refresh = async () => {
      const request = ++latestRequest;
      try {
        const automations = await getAutomations();
        if (!disposed && request === latestRequest) setSnapshot({ workspaceId, automations });
      } catch {
        // Recall is optional: failed IPC must not interrupt the conversation.
        if (!disposed && request === latestRequest) setSnapshot(null);
      }
    };

    void refresh();
    window.addEventListener(AUTOMATIONS_UPDATED_EVENT, refresh);
    window.addEventListener('focus', refresh);
    return () => {
      disposed = true;
      window.removeEventListener(AUTOMATIONS_UPDATED_EVENT, refresh);
      window.removeEventListener('focus', refresh);
    };
  }, [workspaceId]);

  // Hide the previous space synchronously, before the next effect runs.
  if (!workspaceId || snapshot?.workspaceId !== workspaceId) return null;
  const now = new Date();
  const pending = pendingAutomations(snapshot.automations, workspaceId);
  const triggered = pickTodayTriggeredAutomations(snapshot.automations, now, workspaceId);
  if (!pending.length && !triggered.length) return null;

  return (
    <details key={workspaceId} className="workspace-reminders" open={defaultOpen}>
      <summary className="workspace-reminders-summary">
        <span id={headingId} className="workspace-reminders-title">事项回看</span>
        <span className="workspace-reminders-counts">
          待执行 {pending.length} · 今日已触发 {triggered.length}
        </span>
      </summary>
      <div className="workspace-reminders-content" aria-labelledby={headingId}>
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
      </div>
    </details>
  );
}
