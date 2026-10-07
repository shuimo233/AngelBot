import { useEffect, useState, type CSSProperties } from 'react';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspacesStore } from '$stores/workspaces';
import { useMessagesStore } from '$stores/messages';
import { getContextUsage, type ContextUsage } from '$lib/commands/session';
import { getSessionUsage } from '$lib/commands/usage';
import './ChatHeader.css';

/**
 * 会话头部：标题与上下文抽屉入口。
 * 将本轮输入/输出与上下文占用紧邻工作区抽屉入口展示，便于用户把握会话余量。
 */
export function ChatHeader({ onToggleWorkbench }: { onToggleWorkbench?: () => void }) {
  const activeSession = useSessionsStore((state) => state.activeSession);
  const workspaces = useWorkspacesStore((state) => state.workspaces);
  const activeWorkspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const messages = useMessagesStore((state) => state.messages);
  const messageCount = messages.length;
  const [contextUsage, setContextUsage] = useState<ContextUsage | null>(null);
  const [tokenTotals, setTokenTotals] = useState({ input: 0, output: 0 });
  const sessionId = activeSession?.id;
  const activeWorkspace = workspaces.find((workspace) => workspace.id === activeWorkspaceId);
  const workspaceKindLabel = activeWorkspace?.kind === 'project'
    ? '项目'
    : activeWorkspace?.kind === 'personal' ? '个人空间' : '对话';
  const title = activeWorkspace?.kind === 'project'
    ? activeWorkspace.name
    : activeWorkspace?.kind === 'personal' ? 'AngelBot 日常' : activeSession?.title.trim() || 'AngelBot';

  useEffect(() => {
    if (!sessionId) {
      setContextUsage(null);
      return;
    }
    let cancelled = false;
    getContextUsage(sessionId)
      .then((usage) => { if (!cancelled) setContextUsage(usage); })
      .catch(() => { if (!cancelled) setContextUsage(null); });
    return () => { cancelled = true; };
  }, [sessionId, messageCount]);

  useEffect(() => {
    if (!sessionId) return;
    let cancelled = false;
    getSessionUsage(sessionId).then((entries) => {
      const usageEntries = Array.isArray(entries) ? entries : [];
      if (!cancelled) setTokenTotals(usageEntries.reduce(
        (totals, entry) => ({ input: totals.input + entry.prompt_tokens, output: totals.output + entry.completion_tokens }),
        { input: 0, output: 0 },
      ));
    }).catch(() => { if (!cancelled) setTokenTotals({ input: 0, output: 0 }); });
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

  return (
    <div className="chat-header chat-header-new">
      <div className="chat-header-left">
        <span className="chat-header-workspace-kind">{workspaceKindLabel}</span>
        <h1 className="chat-header-session-title">{title}</h1>
      </div>
      <div className="chat-header-right">
        {sessionId && (
          <span
            className="chat-header-context"
            aria-live="polite"
            title={`本会话：输入 ${tokenTotals.input} tokens，输出 ${tokenTotals.output} tokens；当前上下文 ${usagePercent}%`}
          >
            <span>上下文 {usagePercent}%</span>
            <i style={{ '--context-usage': `${usagePercent}%` } as CSSProperties} />
          </span>
        )}
        {onToggleWorkbench && (
          <button type="button" className="chat-header-workbench-toggle" onClick={onToggleWorkbench} title="打开工作台 (Ctrl+.)" aria-label="打开工作台">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true"><rect x="3" y="3" width="18" height="18" rx="2" /><path d="M15 3v18" /></svg>
            <span>工作台</span>
          </button>
        )}
      </div>
    </div>
  );
}
