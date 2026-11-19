import { useEffect, useState } from 'react';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspacesStore } from '$stores/workspaces';
import { useMessagesStore } from '$stores/messages';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspaceActivityStore } from '$stores/workspaceActivity';
import { WorkspaceReminders } from './WorkspaceReminders';
import { AssistantAvatar } from './AssistantAvatar';
import { getContextUsage, type ContextUsage } from '$lib/commands/session';
import { getSessionUsage } from '$lib/commands/usage';
import './ChatHeader.css';

export function ChatHeader({ onToggleWorkbench, workbenchOpen = false }: { onToggleWorkbench?: () => void; workbenchOpen?: boolean }) {
  const activeSession = useSessionsStore((state) => state.activeSession);
  const workspaces = useWorkspacesStore((state) => state.workspaces);
  const activeWorkspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const messages = useMessagesStore((state) => state.messages);
  const messageCount = messages.length;
  const [contextReport, setContextReport] = useState<{ sessionId: string; usage: ContextUsage } | null>(null);
  const [tokenReport, setTokenReport] = useState<{ sessionId: string; input: number; output: number } | null>(null);
  const sessionId = activeSession?.id;
  const contextUsage = contextReport?.sessionId === sessionId ? contextReport?.usage : undefined;
  const tokenTotals = tokenReport && tokenReport.sessionId === sessionId ? tokenReport : null;
  const activeWorkspace = workspaces.find((workspace) => workspace.id === activeWorkspaceId);
  const attentionCount = useWorkspaceActivityStore((state) =>
    !state.error && state.workspaceId === activeWorkspaceId
    && state.sessionId === activeWorkspace?.activeSessionId
    && state.sessionId === sessionId
    && state.projection?.workspaceId === activeWorkspaceId
      ? state.projection.attention?.openCount ?? 0 : 0);
  const workspaceKindLabel = activeWorkspace?.kind === 'project' ? '项目' : '对话';
  const showAssistantAvatar = activeWorkspace?.kind === 'personal' && messages.some(message => message.sessionId === sessionId);
  const title = activeWorkspace?.kind === 'project'
    ? activeWorkspace.name
    : activeWorkspace?.kind === 'personal' ? 'AngelBot 日常' : activeSession?.title.trim() || 'AngelBot';

  useEffect(() => {
    if (!sessionId) return;
    let cancelled = false;
    getContextUsage(sessionId)
      .then((usage) => { if (!cancelled) setContextReport({ sessionId, usage }); })
      .catch(() => { if (!cancelled) setContextReport(null); });
    return () => { cancelled = true; };
  }, [sessionId, messageCount]);

  useEffect(() => {
    if (!sessionId) return;
    let cancelled = false;
    getSessionUsage(sessionId).then((entries) => {
      const usageEntries = Array.isArray(entries) ? entries : [];
      if (!cancelled) setTokenReport(usageEntries.reduce(
        (totals, entry) => ({ ...totals, input: totals.input + entry.prompt_tokens, output: totals.output + entry.completion_tokens }),
        { sessionId, input: 0, output: 0 },
      ));
    }).catch(() => { if (!cancelled) setTokenReport(null); });
    return () => { cancelled = true; };
  }, [sessionId, messageCount]);

  // Keep the indicator visible even while IPC is unavailable (for example
  // during startup or a temporary backend reconnect). The server's value
  // replaces this conservative local estimate as soon as it arrives.
  const fallbackTokens = messages.reduce(
    (total, message) => total + Math.ceil(message.content.length / 4) + 10,
    0,
  );
  const contextLimit = contextUsage?.contextLimit || 128_000;
  const estimatedTokens = contextUsage?.estimatedTokens ?? fallbackTokens;
  const usagePercent = Math.min(100, Math.round((estimatedTokens / contextLimit) * 100));
  const showUsagePercent = activeWorkspace?.kind !== 'personal' || usagePercent >= 80;

  return (
    <div className="chat-header chat-header-new">
      <div className={`chat-header-identity${showAssistantAvatar ? ' chat-header-identity--personal' : ''}`}>
        {showAssistantAvatar && <AssistantAvatar small />}
        {activeWorkspace?.kind !== 'personal' && <span className="chat-header-workspace-kind">{workspaceKindLabel}</span>}
        <h1 className="chat-header-session-title">{title}</h1>
      </div>
      <div className="chat-header-right">
        <WorkspaceReminders workspaceId={activeWorkspace?.id} compact />
        {sessionId && (
          <details
            key={sessionId}
            className="chat-header-context"
            onBlur={(event) => {
              if (!event.currentTarget.contains(event.relatedTarget)) event.currentTarget.open = false;
            }}
            onKeyDown={(event) => {
              if (event.key === 'Escape' && event.currentTarget.open) {
                event.preventDefault();
                event.stopPropagation();
                event.currentTarget.open = false;
                event.currentTarget.querySelector('summary')?.focus();
              }
            }}
          >
            <summary className="chat-header-action chat-header-context-trigger" aria-label={showUsagePercent ? `对话用量，当前上下文 ${usagePercent}%` : '对话用量'} title="对话用量">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
                <path d="M3 19h18M7 15v-4m5 4V7m5 8V4" />
              </svg>
              {showUsagePercent && <small className={usagePercent >= 80 ? 'is-near-limit' : undefined}>{usagePercent}%</small>}
            </summary>
            <div className="chat-header-context-panel">
              <strong>对话用量</strong>
              <p>当前上下文 {usagePercent}%</p>
              <progress aria-label="当前上下文占用" max={100} value={usagePercent} />
              {tokenTotals && <p>本对话输入 {tokenTotals.input.toLocaleString()} · 输出 {tokenTotals.output.toLocaleString()} tokens</p>}
            </div>
          </details>
        )}
        {activeWorkspace?.kind === 'project' && (
          <button type="button" className="chat-header-action" title="所有项目" aria-label="所有项目" onClick={() => useNavigationStore.getState().setPage('projects')}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true"><path d="M3 6a2 2 0 0 1 2-2h4l3 3h7a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /></svg>
            <span>所有项目</span>
          </button>
        )}
        {onToggleWorkbench && (
          <button type="button" className="chat-header-action chat-header-workbench" onClick={onToggleWorkbench} title={`${workbenchOpen ? '收起' : '打开'}工作台 (Ctrl+.)`} aria-label={workbenchOpen ? '收起工作台' : '打开工作台'} aria-expanded={workbenchOpen} aria-controls="conversation-workbench"
            aria-keyshortcuts="Control+." aria-description={attentionCount ? `${attentionCount} 条任务提醒` : undefined}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true"><rect x="3" y="3" width="18" height="18" rx="2" /><path d="M15 3v18" /></svg>
            <span>工作台</span>
            {attentionCount > 0 && <small className="chat-header-attention-count" title={`${attentionCount} 条任务提醒`}>{attentionCount}</small>}
          </button>
        )}
      </div>
    </div>
  );
}
