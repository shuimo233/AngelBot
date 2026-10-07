import { useEffect, useMemo, useState } from 'react';
import { useMessagesStore } from '$stores/messages';
import { useSettingsStore } from '$stores/settings';
import { getMemoryStats, type MemoryStats } from '$lib/commands/memory';
import './WorkflowStatusBar.css';

interface ActiveStep {
  tool: string;
  status: 'running' | 'done' | 'failed';
  output?: string;
}

export function WorkflowStatusBar({ expanded }: { expanded: boolean }) {
  const messages = useMessagesStore((s) => s.messages);
  const profile = useSettingsStore((s) => s.profile);
  const [memoryStats, setMemoryStats] = useState<MemoryStats | null>(null);
  const [activeTools, setActiveTools] = useState<ActiveStep[]>([]);

  // Reload memory stats when new messages arrive
  useEffect(() => {
    getMemoryStats()
      .then(setMemoryStats)
      .catch(() => setMemoryStats(null));
  }, [messages.length]);

  // Extract active tool calls from latest assistant message
  useEffect(() => {
    const lastMsg = messages[messages.length - 1];
    if (!lastMsg || lastMsg.role !== 'assistant' || !lastMsg.toolCalls?.length) {
      setActiveTools([]);
      return;
    }
    const steps: ActiveStep[] = lastMsg.toolCalls.map((tc) => {
      const result = lastMsg.toolResults?.find((r) => r.callId === tc.id);
      return {
        tool: tc.name,
        status: result
          ? result.success ? 'done' : 'failed'
          : 'running',
        output: result?.output,
      };
    });
    setActiveTools(steps);
  }, [messages]);

  /** Modified-file summary from the latest assistant's task facts. The
   *  count mirrors `taskFacts.modifiedFiles.length` so users can see
   *  what the agent changed this turn at a glance, without opening
   *  the right-side task timeline. */
  const modifiedCount = useMemo(() => {
    for (let i = messages.length - 1; i >= 0; i--) {
      const msg = messages[i];
      if (msg.role !== 'assistant') continue;
      return msg.taskFacts?.modifiedFiles?.length ?? 0;
    }
    return 0;
  }, [messages]);

  if (!expanded) return null;

  return (
    <aside className="workflow-status-bar">
      {/* Agent Steps */}
      <div className="workflow-status-section">
        <div className="workflow-status-header">
          <span className="workflow-status-title">当前操作</span>
          <span className="workflow-status-badge">{activeTools.length}</span>
        </div>
        <div className="workflow-steps-list">
          {activeTools.length === 0 ? (
            <div className="workflow-empty">等待开始处理</div>
          ) : (
            activeTools.map((step, i) => (
              <div key={i} className={`workflow-step ${step.status}`}>
                <span className="workflow-step-indicator">
                  <span className="workflow-step-dot" aria-label={step.status === 'running' ? '执行中' : step.status === 'done' ? '已完成' : '失败'} />
                </span>
                <span className="workflow-step-name">{step.tool}</span>
              </div>
            ))
          )}
        </div>
        {modifiedCount > 0 && (
          <p className="workflow-modified-summary">修改 {modifiedCount} 个文件</p>
        )}
      </div>

      {/* Memory */}
      <div className="workflow-status-section">
        <div className="workflow-status-header">
          <span className="workflow-status-title">记忆</span>
          <span className="workflow-status-badge">{memoryStats?.active ?? '—'}</span>
        </div>
        {memoryStats && (
          <div className="workflow-memory-stats">
            <div className="workflow-mem-row">
              <span>长期</span>
              <span className="workflow-mem-value">{memoryStats.permanent}</span>
            </div>
            <div className="workflow-mem-row">
              <span>Summarized</span>
              <span className="workflow-mem-value">{memoryStats.summarized}</span>
            </div>
            <div className="workflow-mem-row">
              <span>Archived</span>
              <span className="workflow-mem-value">{memoryStats.archived}</span>
            </div>
          </div>
        )}
      </div>

      {/* Persona */}
      <div className="workflow-status-section">
        <div className="workflow-status-header">
          <span className="workflow-status-title">Persona</span>
        </div>
        <div className="workflow-persona-preview">
          <div className="workflow-persona-name">{profile.name || '未设置'}</div>
          <div className="workflow-persona-bio">
            {profile.bio ? (profile.bio.length > 80 ? profile.bio.slice(0, 80) + '…' : profile.bio) : '—'}
          </div>
        </div>
      </div>
    </aside>
  );
}
