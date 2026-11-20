import { useEffect, useMemo, useState } from 'react';
import { dismissWorkspaceTaskReminders } from '$lib/commands/workspace-activity';
import { useMessagesStore } from '$stores/messages';
import { useWorkspacesStore } from '$stores/workspaces';
import { useWorkspaceActivityStore } from '$stores/workspaceActivity';
import { CompletedFileTask, latestCompletedFileTask } from './CompletedFileTask';
import { latestResultToReview } from './ChatArea/pendingResultReview';

export function WorkspaceActivityCapsule({
  mode = 'capsule',
  onReturnToConversation,
  onReturnToResultReview,
  reviewInConversation = false,
  workbenchOpen = false,
  onOpenTasks,
}: {
  mode?: 'capsule' | 'panel';
  onReturnToConversation?: () => void;
  onReturnToResultReview?: (messageId: string) => void;
  reviewInConversation?: boolean;
  workbenchOpen?: boolean;
  onOpenTasks?: () => void;
}) {
  const workspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const sessionId = useWorkspacesStore((state) => state.workspaces.find((item) => item.id === workspaceId)?.activeSessionId);
  const messages = useMessagesStore((state) => mode === 'panel' ? state.messages : null);
  const completedFileTask = useMemo(
    () => messages && sessionId ? latestCompletedFileTask(messages, sessionId) : null,
    [messages, sessionId],
  );
  const resultToCheck = useMemo(
    () => messages ? latestResultToReview(messages, sessionId) : undefined,
    [messages, sessionId],
  );
  const projection = useWorkspaceActivityStore((state) => state.projection);
  const refresh = useWorkspaceActivityStore((state) => state.refresh);
  const observe = useWorkspaceActivityStore((state) => state.observe);
  const scopeMatches = useWorkspaceActivityStore((state) => state.workspaceId === workspaceId && state.sessionId === sessionId);
  const isLoading = useWorkspaceActivityStore((state) => state.isLoading) && scopeMatches;
  const error = useWorkspaceActivityStore((state) => state.error);
  const visibleError = scopeMatches ? error : null;
  const [dismissing, setDismissing] = useState(false);
  const [dismissError, setDismissError] = useState<{ workspaceId: string; text: string } | null>(null);

  useEffect(() => {
    if (workspaceId && sessionId) return observe(workspaceId, sessionId);
  }, [observe, workspaceId, sessionId]);

  // Do not project a previous session's child activity while the store is
  // refreshing for the newly selected Workspace.
  const visibleProjection = scopeMatches && projection?.workspaceId === workspaceId ? projection : null;
  const work = visibleProjection?.work ?? [];
  const pending = visibleProjection?.pendingDecisions.length ?? 0;
  const attention = visibleProjection?.attention ?? null;
  const attentionCount = attention?.openCount ?? 0;
  const retryReminderIds = attention?.retryReminderIds ?? [];
  const retriesOnly = attentionCount > 0 && retryReminderIds.length === attentionCount;
  const unsettledWork = work.filter((item) => !isTerminalWorkStatus(item.status));
  const unknown = unsettledWork.filter((item) => item.status === 'unknown').length;
  const active = unsettledWork.length - unknown;
  const pendingDecisions = visibleProjection?.pendingDecisions ?? [];

  const dismissReminders = async () => {
    if (!workspaceId || !sessionId || !retryReminderIds.length || dismissing) return;
    setDismissing(true);
    setDismissError(null);
    try {
      await dismissWorkspaceTaskReminders(workspaceId, sessionId, retryReminderIds);
      await refresh(workspaceId);
    } catch (reason) {
      setDismissError({ workspaceId, text: reason instanceof Error ? reason.message
        : typeof reason === 'string' ? reason : '无法关闭任务提醒，请重试。' });
    } finally {
      setDismissing(false);
    }
  };

  if (mode === 'capsule' && !visibleProjection && visibleError) {
    return (
      <section
        className="workspace-activity-capsule workspace-activity-capsule--error"
        role="alert"
        aria-live="polite"
      >
        <span className="workspace-activity-capsule__dot" aria-hidden="true" />
        <div className="workspace-activity-capsule__body">
          <strong>无法读取任务状态</strong>
          <span>{visibleError}</span>
        </div>
        {workspaceId && (
          <button
            className="workspace-activity-capsule__retry"
            type="button"
            onClick={() => void refresh(workspaceId)}
          >
            重新检查
          </button>
        )}
      </section>
    );
  }
  // Historical Main work remains in the workbench and its header count. Do not
  // keep an old recovery card above the composer or flash loading on each visit.
  if (mode === 'capsule' && unsettledWork.length === 0 && pending === 0 && !visibleError) return null;

  const label = pending > 0
    ? `AngelBot 有 ${pending} 项需要和你确认`
    : active > 0 ? `AngelBot 正在处理 ${active} 项工作`
      : unknown > 0 ? `AngelBot 有 ${unknown} 项工作状态未确认`
        : `AngelBot 有 ${attentionCount} 项工作需要继续处理`;

  if (mode === 'panel') {
    const shownDetails = new Map(work.slice(0, 8).map((item) => [
      item.id, workDetailLabel(item.status, item.reviewerStatus, item.activity, item.summary),
    ]));
    return (
      <section className="workspace-activity-panel" aria-label="任务与委派">
        <div className="workspace-activity-panel__header">
          <h2>任务与委派</h2>
          <button type="button" onClick={() => workspaceId && void refresh(workspaceId)} disabled={!workspaceId || isLoading}>
            {isLoading ? '刷新中…' : '刷新'}
          </button>
        </div>
        {!workspaceId && (
          <div className="workspace-activity-panel__empty">
            <strong>尚未选择工作区</strong>
          </div>
        )}
        {workspaceId && visibleError && (
          <div className="workspace-activity-panel__error" role="alert">
            <strong>无法读取任务状态</strong>
            <span>{visibleError}；以下如有内容，仅为上次记录，不代表任务仍在执行。</span>
            <button type="button" onClick={() => void refresh(workspaceId)}>重新检查</button>
          </div>
        )}
        {workspaceId && !visibleError && !isLoading && work.length === 0 && pending === 0 && attentionCount === 0 && !completedFileTask && !(resultToCheck && onReturnToResultReview) && (
          <div className="workspace-activity-panel__empty">
            <strong>当前没有进行中的工作</strong>
          </div>
        )}
        {resultToCheck && onReturnToResultReview && (
          <div className="workspace-activity-panel__attention">
            {resultToCheck.taskRun?.goal.trim() && <p>{resultToCheck.taskRun.goal.trim().slice(0, 160)}</p>}
            <button type="button" onClick={() => onReturnToResultReview(resultToCheck.id)}>回对话核对结果</button>
          </div>
        )}
        {workspaceId && attention && (
          <div className="workspace-activity-panel__attention" role="status">
            <h3>{retriesOnly ? '执行失败提醒' : '需要继续处理'}（{attention.openCount}）</h3>
            <p>{retriesOnly ? '这些任务此前执行失败。可以回到对话查看记录。' : attention.message}</p>
            <div className="workspace-activity-panel__reminder-actions">
              {onReturnToConversation && <button type="button" onClick={onReturnToConversation}>回到对话</button>}
              {!visibleError && retryReminderIds.length > 0 && <button type="button" disabled={dismissing || isLoading}
                onClick={() => void dismissReminders()}>{dismissing ? '正在关闭提醒…' : retryReminderIds.length > 1 ? '关闭这批失败提醒' : '不再提醒'}</button>}
            </div>
            {dismissError?.workspaceId === workspaceId && <p role="alert">{dismissError.text}</p>}
          </div>
        )}
        {workspaceId && work.length > 0 && (
          <ol className="workspace-activity-panel__list">
            {work.slice(0, 8).map((item) => (
              <li key={item.id} className={`is-${item.status}`}>
                <div className="workspace-activity-panel__item-head">
                  <strong>{item.goal}</strong>
                  <span>{workStageLabel(item.status, item.reviewerStatus, 'compact')}</span>
                </div>
                <p>{workDetailLabel(item.status, item.reviewerStatus, item.activity, item.summary)}</p>
                {(item.milestonesTotal > 0 || item.verificationsTotal > 0) && (
                  <div className="workspace-activity-panel__evidence">
                    {item.milestonesTotal > 0 && <span>步骤 {item.milestonesCompleted}/{item.milestonesTotal}</span>}
                    {item.verificationsTotal > 0 && <span>验证 {item.verificationsPassed}/{item.verificationsTotal}</span>}
                    {item.evidenceCount > 0 && <span>证据 {item.evidenceCount}</span>}
                  </div>
                )}
              </li>
            ))}
          </ol>
        )}
        {workspaceId && pendingDecisions.length > 0 && (
          <div className="workspace-activity-panel__decisions">
            <h3>需要你确认</h3>
            {pendingDecisions.map((decision) => (
              <article key={decision.workId}>
                <strong>{decision.goal}</strong>
                {decision.questions.filter((question) => question !== shownDetails.get(decision.workId))
                  .map((question) => <p key={question}>{question}</p>)}
                {decision.risks.map((risk) => <p key={risk}>风险：{risk}</p>)}
              </article>
            ))}
          </div>
        )}
        {completedFileTask && <CompletedFileTask key={completedFileTask.id} task={completedFileTask} />}
      </section>
    );
  }

  const compactReview = reviewInConversation && Boolean(onOpenTasks);
  if (compactReview && unknown > 0 && active === 0 && pending === 0 && !visibleError) {
    if (workbenchOpen) return null;
    return (
      <div className="workspace-activity-capsule workspace-activity-capsule--shortcut">
        <button className="workspace-activity-capsule__retry" type="button" onClick={onOpenTasks}>查看任务状态（{unknown} 项）</button>
      </div>
    );
  }

  return (
    <section className="workspace-activity-capsule" role="status" aria-live="polite">
      {(pending > 0 || active > 0) && <span className="workspace-activity-capsule__dot" aria-hidden="true" />}
      <div className="workspace-activity-capsule__body">
        {visibleError && <strong role="alert">任务状态暂未更新；以下为上次记录，不代表仍在执行。</strong>}
        <span>{visibleError ? `上次记录：${label}` : label}</span>
        {unknown > 0 && (pending > 0 || active > 0) && !compactReview && <span>另有 {unknown} 项工作状态未确认</span>}
        {unknown > 0 && onOpenTasks && !visibleError && <button className="workspace-activity-capsule__retry" type="button" onClick={onOpenTasks}>查看任务状态（{unknown} 项）</button>}
        {visibleError && workspaceId && <button type="button" onClick={() => void refresh(workspaceId)}>重新检查</button>}
        {unsettledWork.filter((item) => !compactReview || visibleError || item.status !== 'unknown').slice(0, 3).map((item) => (
          <div className="workspace-activity-capsule__item" key={item.id}>
            <span className="workspace-activity-capsule__goal">{item.goal}</span>
            <span className="workspace-activity-capsule__stage">
              {reviewerStatusLabelForActiveWork(item.status, item.reviewerStatus, 'compact')
                ?? (item.status === 'needsDecision' || item.status === 'unknown' ? statusLabel(item.status) : item.activity ?? statusLabel(item.status))}
            </span>
          </div>
        ))}
        {pendingDecisions.map((decision) => (
          <div className="workspace-activity-capsule__decision" key={decision.workId}>
            <span className="workspace-activity-capsule__goal">{decision.goal}</span>
            {decision.questions.map((question) => (
              <span className="workspace-activity-capsule__decision-detail" key={`question-${question}`}>
                {question}
              </span>
            ))}
            {decision.risks.map((risk) => (
              <span className="workspace-activity-capsule__decision-detail" key={`risk-${risk}`}>
                风险：{risk}
              </span>
            ))}
          </div>
        ))}
      </div>
    </section>
  );
}

function statusLabel(status: string): string {
  const labels: Record<string, string> = {
    queued: '等待分配',
    running: '正在执行',
    awaitingConfirmation: '等待确认',
    awaitingSummary: '等待主 Agent 汇总',
    awaitingReview: '独立审校中',
    materializationPending: '等待写入',
    needsDecision: '需要处理',
    completed: '已完成',
    failed: '未完成',
    cancelled: '已取消',
    unknown: '状态未确认',
  };
  return labels[status] ?? '处理中';
}

function isTerminalWorkStatus(status: string): boolean {
  return ['completed', 'failed', 'cancelled'].includes(status);
}

function workStageLabel(status: string, reviewerStatus: string | null, mode: 'detail' | 'compact'): string {
  return reviewerStatusLabelForActiveWork(status, reviewerStatus, mode) ?? statusLabel(status);
}

function workDetailLabel(
  status: string,
  reviewerStatus: string | null,
  activity: string | null,
  summary: string | null,
): string {
  if (isTerminalWorkStatus(status)) return terminalWorkDetail(status);
  if (status === 'unknown') return '暂时无法确认这项工作是否仍在执行，请刷新后核对。';
  if (status === 'needsDecision' && !reviewerStatus) return summary ?? '执行需要处理，暂无可交付结果';
  return reviewerStatusLabelForActiveWork(status, reviewerStatus) ?? activity ?? summary ?? '等待下一步状态';
}

function terminalWorkDetail(status: string): string {
  const labels: Record<string, string> = {
    completed: '任务已完成',
    failed: '任务未完成',
    cancelled: '任务已取消',
  };
  return labels[status] ?? statusLabel(status);
}

function reviewerStatusLabelForActiveWork(
  status: string,
  reviewerStatus: string | null,
  mode: 'detail' | 'compact' = 'detail',
): string | null {
  return isTerminalWorkStatus(status) || status === 'unknown' ? null : reviewerStatusLabel(reviewerStatus, mode);
}

function reviewerStatusLabel(status: string | null, mode: 'detail' | 'compact' = 'detail'): string | null {
  const labels: Record<string, [string, string]> = {
    queued: ['独立审校正在等待开始', '独立审校等待中'],
    running: ['独立审校正在检查结果', '独立审校中'],
    passed: ['独立审校已通过，等待主 Agent 处理', '独立审校已通过'],
    failed: ['独立审校未通过，需要主 Agent 处理', '独立审校未通过'],
    needs_decision: ['独立审校需要判断', '审校需要判断'],
    cancelled: ['独立审校已取消', '独立审校已取消'],
  };
  return status ? labels[status]?.[mode === 'compact' ? 1 : 0] ?? null : null;
}
