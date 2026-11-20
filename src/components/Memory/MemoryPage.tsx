import { useEffect, useMemo, useRef, useState } from 'react';
import { getMemories, type RustMemory } from '$lib/commands/memory';
import { MemoryGovernanceSettings } from '../Settings/pages/MemoryGovernanceSettings';
import { memoryCategoryLabel } from './memoryPresentation';
import './MemoryPage.css';

export function MemoryPage() {
  const [memories, setMemories] = useState<RustMemory[]>([]);
  const [showGovernance, setShowGovernance] = useState(false);
  const [focusedMemoryId, setFocusedMemoryId] = useState<string>();
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const backButton = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (showGovernance) backButton.current?.focus();
  }, [showGovernance]);

  useEffect(() => {
    if (showGovernance) return;
    let disposed = false;
    setLoading(true); setError(null);
    getMemories().then(items => { if (!disposed) setMemories(items); })
      .catch(() => { if (!disposed) setError('无法读取本地记忆，请稍后重新打开。'); })
      .finally(() => { if (!disposed) setLoading(false); });
    return () => { disposed = true; };
  }, [showGovernance]);

  const personalMemories = useMemo(() => memories.filter(memory => memory.category !== 'knowledge' && memory.source !== 'knowledge'), [memories]);
  const activeMemories = useMemo(() => personalMemories.filter((memory) => memory.forget_stage === 'active'), [personalMemories]);

  if (showGovernance) {
    return <div className="memory-page memory-governance-page">
      <div className="memory-hero"><h2>{focusedMemoryId ? '查看与更正记忆' : '管理记忆'}</h2><button ref={backButton} className="memory-back-button" type="button" onClick={() => { setFocusedMemoryId(undefined); setShowGovernance(false); }}>返回记忆概览</button></div>
      <MemoryGovernanceSettings focusedMemoryId={focusedMemoryId} />
    </div>;
  }

  return <div className="memory-page">
    <section className="memory-hero" aria-label="记忆概览">
      <div className="memory-overview-stats">
        {!loading && !error && personalMemories.length > 0 && (
          <span><strong>{personalMemories.length}</strong> 条记忆{activeMemories.length !== personalMemories.length && ` · ${activeMemories.length} 条可用`}</span>
        )}
      </div>
      <div className="memory-hero-actions">
        <button type="button" onClick={() => setShowGovernance(true)}>管理记忆</button>
      </div>
    </section>
    {error && <p className="memory-page-error" role="alert">{error}</p>}
    {loading && <p role="status">正在读取记忆…</p>}
    {!loading && !error && personalMemories.length === 0 && <p className="memory-empty-state">还没有可显示的长期记忆</p>}
    {!loading && !error && personalMemories.length > 0 && <>
      {activeMemories.length === 0 ? <p className="memory-archived-note">已暂停或归档的内容可在「管理记忆」查看。</p> : <div className="memory-preview-grid">
        {activeMemories.slice(0, 8).map((memory) => (
          <article key={memory.id} className="memory-preview-card">
            <div><span className="memory-category">{memoryCategoryLabel(memory.category)}</span>{memory.is_permanent && <span className="memory-permanent">长期保留</span>}</div>
            <p>{memory.content}</p>
            <button type="button" className="ui-button ui-button--secondary ui-button--small" onClick={() => { setFocusedMemoryId(memory.id); setShowGovernance(true); }}>查看与更正</button>
          </article>
        ))}
      </div>}
    </>}
  </div>;
}
