import { useEffect, useMemo, useState } from 'react';
import { getMemories, type RustMemory } from '$lib/commands/memory';
import { MemoryGovernanceSettings } from '../Settings/pages/MemoryGovernanceSettings';
import './MemoryPage.css';

export function MemoryPage() {
  const [memories, setMemories] = useState<RustMemory[]>([]);
  const [showGovernance, setShowGovernance] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getMemories().then(setMemories).catch(() => setError('无法读取本地记忆。'));
  }, []);

  const activeMemories = useMemo(() => memories.filter((memory) => memory.forget_stage === 'active'), [memories]);
  const permanentCount = useMemo(() => memories.filter((memory) => memory.is_permanent).length, [memories]);

  if (showGovernance) {
    return <div className="memory-page memory-governance-page">
      <button className="memory-back-button" type="button" onClick={() => setShowGovernance(false)}>返回记忆概览</button>
      <MemoryGovernanceSettings />
    </div>;
  }

  return <div className="memory-page">
    <section className="memory-hero">
      <div>
        <span className="memory-eyebrow">长期记忆</span>
        <h2>记忆与偏好</h2>
        <p>AngelBot 从交流中保留的重要偏好和事实，所有内容都可审计、编辑与治理。</p>
      </div>
      <div className="memory-hero-actions">
        <div className="memory-stat"><strong>{activeMemories.length}</strong><span>条活跃记忆</span></div>
        <button type="button" onClick={() => setShowGovernance(true)}>查看审计与治理</button>
      </div>
    </section>
    {error && <p className="memory-page-error" role="alert">{error}</p>}
    {!error && memories.length === 0 && <section className="memory-empty-state"><strong>还没有可显示的长期记忆</strong><p>当你们的交流形成值得保留的信息时，它会出现在这里。</p></section>}
    {!error && memories.length > 0 && <>
      <div className="memory-overview-stats"><span><strong>{activeMemories.length}</strong> 活跃</span><span><strong>{permanentCount}</strong> 永久保留</span><span><strong>{memories.length}</strong> 全部记忆</span></div>
      <div className="memory-preview-grid">{activeMemories.slice(0, 8).map((memory) => <article key={memory.id} className="memory-preview-card"><div><span className="memory-category">{memory.category || 'general'}</span>{memory.is_permanent && <span className="memory-permanent">永久</span>}</div><p>{memory.content}</p><footer><span>重要度 {memory.importance}/7</span><span>{memory.source}</span></footer></article>)}</div>
    </>}
  </div>;
}
