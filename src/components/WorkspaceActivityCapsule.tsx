import { useEffect, useMemo } from 'react';
import { useWorkspacesStore } from '$stores/workspaces';
import { useWorkspaceActivityStore } from '$stores/workspaceActivity';

export function WorkspaceActivityCapsule({
  mode = 'capsule',
  onReturnToConversation,
}: {
  mode?: 'capsule' | 'panel';
  onReturnToConversation?: () => void;
}) {
  const workspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const sessionId = useWorkspacesStore((state) => state.workspaces.find((item) => item.id === workspaceId)?.activeSessionId);
  const projection = useWorkspaceActivityStore((state) => state.projection);
  const refresh = useWorkspaceActivityStore((state) => state.refresh);
  const observe = useWorkspaceActivityStore((state) => state.observe);
  const scopeMatches = useWorkspaceActivityStore((state) => state.workspaceId === workspaceId && state.sessionId === sessionId);
  const isLoading = useWorkspaceActivityStore((state) => state.isLoading) && scopeMatches;
  const error = useWorkspaceActivityStore((state) => state.error);
  const visibleError = scopeMatches ? error : null;

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
  const active = work.filter((item) => !['completed', 'failed', 'cancelled'].includes(item.status)).length;
  const activeWork = useMemo(
    () => work.filter((item) => !['completed', 'failed', 'cancelled'].includes(item.status)).slice(0, 3),
    [work],
  );
  const pendingDecisions = visibleProjection?.pendingDecisions ?? [];

  if (mode === 'capsule' && !visibleProjection && (visibleError || isLoading)) {
    return (
      <section
        className={`workspace-activity-capsule${visibleError ? ' workspace-activity-capsule--error' : ''}`}
        role={visibleError ? 'alert' : 'status'}
        aria-live="polite"
      >
        <span className="workspace-activity-capsule__dot" aria-hidden="true" />
        <div className="workspace-activity-capsule__body">
          <strong>{visibleError ? '无法读取任务状态' : '正在同步任务状态'}</strong>
          {visibleError && <span>{visibleError}</span>}
        </div>
        {visibleError && workspaceId && (
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
  if (mode === 'capsule' && active === 0 && pending === 0 && attentionCount === 0 && !visibleError) return null;

  const label = pending > 0
    ? `AngelBot 有 ${pending} 项需要和你确认`
    : attentionCount > 0
      ? `AngelBot 有 ${attentionCount} 项工作需要继续处理`
    : `AngelBot 正在处理 ${active} 项工作`;

  if (mode === 'panel') {
    const shownDetails = new Map(work.slice(0, 8).map((item) => [
      item.id, workDetailLabel(item.status, item.reviewerStatus, item.activity, item.summary),
    ]));
    return (
      <section className="workspace-activity-panel" aria-label="任务与委派">
        <div className="workspace-activity-panel__header">
          <div>
            <span className="workspace-activity-panel__eyebrow">实时工作状态</span>
            <h2>任务与委派</h2>
          </div>
          <button type="button" onClick={() => workspaceId && void refresh(workspaceId)} disabled={!workspaceId || isLoading}>
            {isLoading ? '刷新中…' : '刷新'}
          </button>
        </div>
        {!workspaceId && (
          <div className="workspace-activity-panel__empty">
            <strong>尚未选择工作区</strong>
            <span>选择个人空间或项目后查看对应工作。</span>
          </div>
        )}
        {workspaceId && visibleError && (
          <div className="workspace-activity-panel__error" role="alert">
            <strong>无法读取任务状态</strong>
            <span>{visibleError}；以下如有内容，仅为上次记录，不代表任务仍在执行。</span>
            <button type="button" onClick={() => void refresh(workspaceId)}>重新检查</button>
          </div>
        )}
        {workspaceId && !visibleError && !isLoading && work.length === 0 && pending === 0 && attentionCount === 0 && (
          <div className="workspace-activity-panel__empty">
            <strong>当前没有进行中的工作</strong>
            <span>让主 Agent 处理复杂任务时，委派、审校、待确认和需要继续处理的事项会显示在这里。</span>
          </div>
        )}
        {workspaceId && attention && (
          <div className="workspace-activity-panel__attention" role="status">
            <h3>需要继续处理（{attention.openCount}）</h3>
            <p>{attention.message}</p>
            {onReturnToConversation && (
              <button type="button" onClick={onReturnToConversation}>回到对话</button>
            )}
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
      </section>
    );
  }

  return (
    <section className="workspace-activity-capsule" role="status" aria-live="polite">
      <span className="workspace-activity-capsule__dot" aria-hidden="true" />
      <div className="workspace-activity-capsule__body">
        {visibleError && <strong role="alert">任务状态暂未更新；以下为上次记录，不代表仍在执行。</strong>}
        <span>{visibleError ? `上次记录：${label}` : label}</span>
        {visibleError && workspaceId && <button type="button" onClick={() => void refresh(workspaceId)}>重新检查</button>}
        {activeWork.map((item) => (
          <div className="workspace-activity-capsule__item" key={item.id}>
            <span className="workspace-activity-capsule__goal">{item.goal}</span>
            <span className="workspace-activity-capsule__stage">
              {reviewerStatusLabelForActiveWork(item.status, item.reviewerStatus, 'compact')
                ?? (item.status === 'needsDecision' ? statusLabel(item.status) : item.activity ?? statusLabel(item.status))}
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
        {attention && (
          <div className="workspace-activity-capsule__attention">
            <span className="workspace-activity-capsule__attention-detail">
              进度已安全保留，可直接在对话中继续。
            </span>
          </div>
        )}
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
  return isTerminalWorkStatus(status) ? null : reviewerStatusLabel(reviewerStatus, mode);
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
