import { useEffect, useMemo, useState } from 'react';
import { getMemories, type RustMemory } from '$lib/commands/memory';
import { MemoryGovernanceSettings } from '../Settings/pages/MemoryGovernanceSettings';
import './MemoryPage.css';

const categoryLabels: Record<string, string> = { preference: '偏好', fact: '事实', event: '事项', knowledge: '参考资料', personality: '相处方式' };

export function MemoryPage() {
  const [memories, setMemories] = useState<RustMemory[]>([]);
  const [showGovernance, setShowGovernance] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

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
  const permanentCount = useMemo(() => personalMemories.filter((memory) => memory.is_permanent).length, [personalMemories]);

  if (showGovernance) {
    return <div className="memory-page memory-governance-page">
      <button className="memory-back-button" type="button" onClick={() => setShowGovernance(false)}>返回记忆概览</button>
      <MemoryGovernanceSettings />
    </div>;
  }

  return <div className="memory-page">
    <section className="memory-hero">
      <div>
        <h2>关于我的记忆</h2>
        <p>交流中保留的偏好和事实，可以在这里检查、修改或移除。</p>
      </div>
      <div className="memory-hero-actions">
        <button type="button" onClick={() => setShowGovernance(true)}>管理记忆</button>
      </div>
    </section>
    {error && <p className="memory-page-error" role="alert">{error}</p>}
    {loading && <p role="status">正在读取记忆…</p>}
    {!loading && !error && personalMemories.length === 0 && <section className="memory-empty-state"><strong>还没有可显示的长期记忆</strong><p>启用记忆后，交流中保留的重要信息会显示在这里。比如你的回答风格偏好，而不是一份需要查阅的文件。</p></section>}
    {!loading && !error && personalMemories.length > 0 && <>
      <div className="memory-overview-stats"><span><strong>{activeMemories.length}</strong> 活跃</span><span><strong>{permanentCount}</strong> 长期保留</span><span><strong>{personalMemories.length}</strong> 全部记忆</span></div>
      {activeMemories.length === 0 ? <p className="memory-archived-note">当前没有活跃记忆，可在“管理记忆”中查看已归档的内容。</p> : <div className="memory-preview-grid">{activeMemories.slice(0, 8).map((memory) => <article key={memory.id} className="memory-preview-card"><div><span className="memory-category">{categoryLabels[memory.category] || '其他记忆'}</span>{memory.is_permanent && <span className="memory-permanent">长期保留</span>}</div><p>{memory.content}</p></article>)}</div>}
    </>}
  </div>;
}
