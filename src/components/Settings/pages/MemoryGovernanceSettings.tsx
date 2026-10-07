import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  deleteMemory,
  detectMemoryConflicts,
  getMemories,
  getMemoryHistory,
  mergeMemories,
  setMemoryPermanent,
  updateMemory,
  type MemoryConflict,
  type MemoryHistoryEntry,
  type RustMemory,
} from '$lib/commands/memory';

type MemoryDraft = {
  scope: string;
  category: string;
  content: string;
  importance: number;
  isPermanent: boolean;
};

const stageOptions = ['all', 'active', 'summarized', 'archived'];

function toDraft(memory: RustMemory): MemoryDraft {
  return {
    scope: memory.scope,
    category: memory.category,
    content: memory.content,
    importance: memory.importance,
    isPermanent: memory.is_permanent,
  };
}

function formatDate(timestamp: number | null) {
  if (!timestamp) return 'Never';
  return new Date(timestamp * 1000).toLocaleString();
}

export function MemoryGovernanceSettings() {
  const [memories, setMemories] = useState<RustMemory[]>([]);
  const [drafts, setDrafts] = useState<Record<string, MemoryDraft>>({});
  const [query, setQuery] = useState('');
  const [categoryFilter, setCategoryFilter] = useState('all');
  const [stageFilter, setStageFilter] = useState('all');
  const [selectedMemoryId, setSelectedMemoryId] = useState<string | null>(null);
  const [history, setHistory] = useState<Record<string, MemoryHistoryEntry[]>>({});
  const [conflicts, setConflicts] = useState<Record<string, MemoryConflict[]>>({});
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadMemories = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const loaded = await getMemories();
      setMemories(loaded);
      setDrafts((prev) => {
        const next = { ...prev };
        for (const memory of loaded) {
          next[memory.id] = next[memory.id] ?? toDraft(memory);
        }
        return next;
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load memories');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadMemories();
  }, [loadMemories]);

  const categories = useMemo(() => {
    return Array.from(new Set(memories.map((m) => m.category))).sort();
  }, [memories]);

  const filteredMemories = useMemo(() => {
    const q = query.trim().toLowerCase();
    return memories.filter((memory) => {
      if (categoryFilter !== 'all' && memory.category !== categoryFilter) return false;
      if (stageFilter !== 'all' && memory.forget_stage !== stageFilter) return false;
      if (!q) return true;
      return [memory.content, memory.category, memory.scope, memory.source]
        .some((value) => value.toLowerCase().includes(q));
    });
  }, [categoryFilter, memories, query, stageFilter]);

  const updateDraft = useCallback((id: string, patch: Partial<MemoryDraft>) => {
    setDrafts((prev) => ({
      ...prev,
      [id]: { ...(prev[id] ?? { scope: 'global', category: 'fact', content: '', importance: 5, isPermanent: false }), ...patch },
    }));
  }, []);

  const saveDraft = useCallback(async (memory: RustMemory) => {
    const draft = drafts[memory.id] ?? toDraft(memory);
    setError(null);
    await updateMemory({
      id: memory.id,
      scope: draft.scope,
      category: draft.category,
      content: draft.content,
      importance: draft.importance,
      isPermanent: draft.isPermanent,
    });
    await loadMemories();
  }, [drafts, loadMemories]);

  const togglePermanent = useCallback(async (memory: RustMemory) => {
    setError(null);
    await setMemoryPermanent(memory.id, !memory.is_permanent);
    setDrafts((prev) => ({
      ...prev,
      [memory.id]: { ...(prev[memory.id] ?? toDraft(memory)), isPermanent: !memory.is_permanent },
    }));
    await loadMemories();
  }, [loadMemories]);

  const removeMemory = useCallback(async (id: string) => {
    setError(null);
    await deleteMemory(id);
    await loadMemories();
  }, [loadMemories]);

  const loadHistory = useCallback(async (id: string) => {
    setSelectedMemoryId((current) => current === id ? null : id);
    if (history[id]) return;
    setError(null);
    const entries = await getMemoryHistory(id);
    setHistory((prev) => ({ ...prev, [id]: entries }));
  }, [history]);

  const checkConflicts = useCallback(async (memory: RustMemory) => {
    const draft = drafts[memory.id] ?? toDraft(memory);
    setError(null);
    const found = await detectMemoryConflicts(draft.content, 0.8);
    setConflicts((prev) => ({
      ...prev,
      [memory.id]: found.filter((conflict) => conflict.memory_id !== memory.id),
    }));
  }, [drafts]);

  const mergeIntoCurrent = useCallback(async (sourceId: string, targetId: string) => {
    setError(null);
    await mergeMemories(sourceId, targetId);
    await loadMemories();
  }, [loadMemories]);

  return (
    <div className="memory-governance">
      <div className="memory-toolbar">
        <input
          className="memory-search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="搜索记忆"
          aria-label="搜索记忆"
        />
        <select value={categoryFilter} onChange={(e) => setCategoryFilter(e.target.value)} aria-label="按类别筛选">
          <option value="all">全部分类</option>
          {categories.map((category) => (
            <option key={category} value={category}>{category}</option>
          ))}
        </select>
        <select value={stageFilter} onChange={(e) => setStageFilter(e.target.value)} aria-label="按阶段筛选">
          {stageOptions.map((stage) => (
            <option key={stage} value={stage}>{stage === 'all' ? '全部阶段' : stage}</option>
          ))}
        </select>
        <button className="btn btn-secondary btn-sm" onClick={loadMemories} disabled={loading}>
          {loading ? '加载中' : '刷新'}
        </button>
      </div>

      {error && <p className="memory-error">{error}</p>}

      <div className="memory-summary-row">
        <span>{filteredMemories.length} 条显示中</span>
        <span>{memories.filter((m) => m.is_permanent).length} 条永久保留</span>
      </div>

      <div className="memory-list">
        {filteredMemories.map((memory) => {
          const draft = drafts[memory.id] ?? toDraft(memory);
          const memoryHistory = history[memory.id] ?? [];
          const memoryConflicts = conflicts[memory.id] ?? [];
          return (
            <section className="memory-item" key={memory.id}>
              <div className="memory-item-header">
                <div className="memory-badges">
                  <span className="memory-badge">{memory.category}</span>
                  <span className="memory-badge muted">{memory.forget_stage}</span>
                  {memory.is_permanent && <span className="memory-badge permanent">Permanent</span>}
                </div>
                <div className="memory-meta">
                  <span>Used {memory.frequency}</span>
                  <span>Updated {formatDate(memory.updated_at)}</span>
                </div>
              </div>

              <textarea
                className="memory-content-input"
                value={draft.content}
                onChange={(e) => updateDraft(memory.id, { content: e.target.value })}
                aria-label={`Memory content ${memory.id}`}
                rows={3}
              />

              <div className="memory-edit-grid">
                <label>
                  Scope
                  <input value={draft.scope} onChange={(e) => updateDraft(memory.id, { scope: e.target.value })} />
                </label>
                <label>
                  Category
                  <input value={draft.category} onChange={(e) => updateDraft(memory.id, { category: e.target.value })} />
                </label>
                <label>
                  Importance
                  <input
                    type="number"
                    min={1}
                    max={10}
                    value={draft.importance}
                    onChange={(e) => updateDraft(memory.id, { importance: Number(e.target.value) })}
                  />
                </label>
                <label className="memory-check">
                  <input
                    type="checkbox"
                    checked={draft.isPermanent}
                    onChange={(e) => updateDraft(memory.id, { isPermanent: e.target.checked })}
                  />
                  Never forget
                </label>
              </div>

              <div className="memory-actions">
                <button className="btn btn-primary btn-sm" onClick={() => saveDraft(memory)}>Save</button>
                <button className="btn btn-secondary btn-sm" onClick={() => togglePermanent(memory)}>
                  {memory.is_permanent ? 'Unpin' : 'Pin'}
                </button>
                <button className="btn btn-secondary btn-sm" onClick={() => loadHistory(memory.id)}>History</button>
                <button className="btn btn-secondary btn-sm" onClick={() => checkConflicts(memory)}>Conflicts</button>
                <button className="btn btn-danger btn-sm" onClick={() => removeMemory(memory.id)}>Delete</button>
              </div>

              {selectedMemoryId === memory.id && (
                <div className="memory-history">
                  {memoryHistory.length === 0 ? (
                    <p>No history yet.</p>
                  ) : memoryHistory.map((entry) => (
                    <div className="memory-history-entry" key={entry.id}>
                      <strong>{entry.operation}</strong>
                      <span>{formatDate(entry.createdAt)}</span>
                      {entry.details && <p>{entry.details}</p>}
                    </div>
                  ))}
                </div>
              )}

              {memoryConflicts.length > 0 && (
                <div className="memory-conflicts">
                  {memoryConflicts.map((conflict) => (
                    <div className="memory-conflict" key={conflict.memory_id}>
                      <div>
                        <strong>{Math.round(conflict.similarity * 100)}% similar</strong>
                        <p>{conflict.content}</p>
                      </div>
                      <button className="btn btn-secondary btn-sm" onClick={() => mergeIntoCurrent(conflict.memory_id, memory.id)}>
                        Merge into this
                      </button>
                    </div>
                  ))}
                </div>
              )}
            </section>
          );
        })}
      </div>

      {!loading && filteredMemories.length === 0 && (
        <div className="memory-empty">没有符合当前筛选条件的记忆。</div>
      )}
    </div>
  );
}
